//! TURZX 3.5" USB 디스플레이(Rev A / UsbMonitor 계열) 프로토콜.
//!
//! 독립 재작성: 커뮤니티에 공개된 프로토콜 사실만 사용했다.
//! - 명령 패킷: 6바이트 = 10비트 x/y/ex/ey(사각형, 양끝 포함) + 명령 바이트
//! - HELLO: 0x45 × 6 — 정식 Turing 3.5는 응답하지 않음(무응답이 정상)
//! - 픽셀: RGB565 little-endian, 청크 = 화면 폭 × 8, 청크마다 flush
//! - 밝기: 장치 값 0=최대, 255=최소 (퍼센트를 반전해 전송)
//! - SET_ORIENTATION: 6바이트 패킷 뒤에 [방향+100, 폭(BE16), 높이(BE16)] 5바이트

pub const CMD_HELLO: u8 = 69;
#[allow(dead_code)] // 진단/복구용(리셋은 포트 재열거를 유발해 사용 안 함)
pub const CMD_RESET: u8 = 101;
pub const CMD_CLEAR: u8 = 102;
#[allow(dead_code)] // 진단용(검정 지우기)
pub const CMD_TO_BLACK: u8 = 103;
pub const CMD_SCREEN_OFF: u8 = 108;
pub const CMD_SCREEN_ON: u8 = 109;
pub const CMD_SET_BRIGHTNESS: u8 = 110;
pub const CMD_SET_ORIENTATION: u8 = 121;
pub const CMD_DISPLAY_BITMAP: u8 = 197;

pub const BAUD_RATE: u32 = 115_200;
pub const WIDTH_PORTRAIT: u16 = 320;
pub const HEIGHT_PORTRAIT: u16 = 480;

/// HELLO 응답으로 구분되는 패널 모델. 무응답/알 수 없는 응답은 Turing 3.5로 본다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelModel {
    Turing35,
    UsbMonitor35,
    UsbMonitor5,
    UsbMonitor7,
}

impl PanelModel {
    pub fn label(self) -> &'static str {
        match self {
            PanelModel::Turing35 => "TURZX 3.5\"",
            PanelModel::UsbMonitor35 => "UsbMonitor 3.5\"",
            PanelModel::UsbMonitor5 => "UsbMonitor 5\"",
            PanelModel::UsbMonitor7 => "UsbMonitor 7\"",
        }
    }

    #[allow(dead_code)] // 5"/7" 모델 감지 시 안내용
    pub fn size(self) -> (u16, u16) {
        match self {
            PanelModel::Turing35 | PanelModel::UsbMonitor35 => (WIDTH_PORTRAIT, HEIGHT_PORTRAIT),
            PanelModel::UsbMonitor5 => (480, 800),
            PanelModel::UsbMonitor7 => (600, 1024),
        }
    }

    /// 320x480 대시보드 레이아웃을 그대로 쓸 수 있는 모델인지(5"/7"는 미지원).
    pub fn is_supported(self) -> bool {
        matches!(self, PanelModel::Turing35 | PanelModel::UsbMonitor35)
    }
}

/// 10비트 좌표 4개 + 명령 바이트를 6바이트로 패킹한다.
pub fn command_packet(command: u8, x: u16, y: u16, ex: u16, ey: u16) -> [u8; 6] {
    [
        ((x >> 2) & 0xFF) as u8,
        ((((x & 0x03) << 6) | (y >> 4)) & 0xFF) as u8,
        ((((y & 0x0F) << 4) | (ex >> 6)) & 0xFF) as u8,
        ((((ex & 0x3F) << 2) | (ey >> 8)) & 0xFF) as u8,
        (ey & 0xFF) as u8,
        command,
    ]
}

pub fn hello_packet() -> [u8; 6] {
    [CMD_HELLO; 6]
}

/// HELLO 응답 해석. [1;6]=UsbMonitor 3.5, [2;6]=5", [3;6]=7", 그 외/무응답=Turing 3.5.
pub fn parse_hello(response: &[u8]) -> PanelModel {
    match response {
        [1, 1, 1, 1, 1, 1] => PanelModel::UsbMonitor35,
        [2, 2, 2, 2, 2, 2] => PanelModel::UsbMonitor5,
        [3, 3, 3, 3, 3, 3] => PanelModel::UsbMonitor7,
        _ => PanelModel::Turing35,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    Landscape,
    ReversePortrait,
    ReverseLandscape,
}

impl Orientation {
    pub fn parse(text: &str) -> Orientation {
        match text.to_lowercase().as_str() {
            "landscape" => Orientation::Landscape,
            "reverse-portrait" => Orientation::ReversePortrait,
            "reverse-landscape" => Orientation::ReverseLandscape,
            _ => Orientation::Portrait,
        }
    }

    pub fn from_rotation(rotation: u16, base: Orientation) -> Orientation {
        match (base, rotation % 360) {
            (Orientation::Portrait, 0) => Orientation::Portrait,
            (Orientation::Portrait, 90) => Orientation::Landscape,
            (Orientation::Portrait, 180) => Orientation::ReversePortrait,
            (Orientation::Portrait, 270) => Orientation::ReverseLandscape,
            (Orientation::Landscape, 0) => Orientation::Landscape,
            (Orientation::Landscape, 90) => Orientation::ReversePortrait,
            (Orientation::Landscape, 180) => Orientation::ReverseLandscape,
            (Orientation::Landscape, 270) => Orientation::Portrait,
            (other, _) => other,
        }
    }

    /// 하드웨어 방향 값(0=정방향 세로, 1=180° 세로, 2=가로, 3=180° 가로).
    pub fn hardware_value(self) -> u8 {
        match self {
            Orientation::Portrait => 0,
            Orientation::ReversePortrait => 1,
            Orientation::Landscape => 2,
            Orientation::ReverseLandscape => 3,
        }
    }

    pub fn size(self) -> (u16, u16) {
        match self {
            Orientation::Portrait | Orientation::ReversePortrait => (WIDTH_PORTRAIT, HEIGHT_PORTRAIT),
            Orientation::Landscape | Orientation::ReverseLandscape => (HEIGHT_PORTRAIT, WIDTH_PORTRAIT),
        }
    }
}

/// SET_ORIENTATION: 11바이트 = 6바이트 패킷 + [방향+100, 폭(BE16), 높이(BE16)].
pub fn orientation_packet(orientation: Orientation) -> [u8; 11] {
    let (width, height) = orientation.size();
    let mut out = [0u8; 11];
    out[..6].copy_from_slice(&command_packet(CMD_SET_ORIENTATION, 0, 0, 0, 0));
    out[6] = orientation.hardware_value() + 100;
    out[7] = (width >> 8) as u8;
    out[8] = (width & 0xFF) as u8;
    out[9] = (height >> 8) as u8;
    out[10] = (height & 0xFF) as u8;
    out
}

/// 밝기(%) → 장치 값(0=최대, 255=최소).
pub fn brightness_value(percent: u8) -> u8 {
    let clamped = percent.min(100);
    (255 - (clamped as u32 * 255) / 100) as u8
}

pub fn brightness_packet(percent: u8) -> [u8; 6] {
    command_packet(CMD_SET_BRIGHTNESS, brightness_value(percent) as u16, 0, 0, 0)
}

pub fn screen_packet(on: bool) -> [u8; 6] {
    command_packet(
        if on { CMD_SCREEN_ON } else { CMD_SCREEN_OFF },
        0,
        0,
        0,
        0,
    )
}

#[allow(dead_code)] // 진단/복구용(화면을 흰색으로)
pub fn clear_packet() -> [u8; 6] {
    command_packet(CMD_CLEAR, 0, 0, 0, 0)
}

/// 이미지 데이터 전송 청크 크기(참조 구현과 동일: 화면 폭 × 8).
pub fn chunk_size(width: u16) -> usize {
    width as usize * 8
}

/// RGB8 → RGB565 little-endian.
pub fn rgb565_le(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 2);
    for pixel in rgb.chunks_exact(3) {
        let r = (pixel[0] as u16) >> 3;
        let g = (pixel[1] as u16) >> 2;
        let b = (pixel[2] as u16) >> 3;
        let value = (r << 11) | (g << 5) | b;
        out.push((value & 0xFF) as u8);
        out.push((value >> 8) as u8);
    }
    out
}

#[allow(dead_code)] // 역방향은 하드웨어가 처리하지만, 진단/대체 경로용으로 유지
/// 180도 회전(가로/세로 180° 방향을 소프트웨어로 처리할 때 사용).
pub fn rotate180(rgb: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; rgb.len()];
    for y in 0..height {
        for x in 0..width {
            let source = (y * width + x) * 3;
            let target = ((height - 1 - y) * width + (width - 1 - x)) * 3;
            out[target..target + 3].copy_from_slice(&rgb[source..source + 3]);
        }
    }
    out
}

/// 이전/현재 프레임을 타일 단위로 비교해 변경 영역의 병합 사각형을 돌려준다.
/// 변경이 없으면 None, 화면 대부분이 바뀌면 전체 프레임(0,0,w-1,h-1)을 돌려준다.
pub fn dirty_rect(
    previous: &[u8],
    current: &[u8],
    width: usize,
    height: usize,
    tile: usize,
) -> Option<(u16, u16, u16, u16)> {
    if previous.len() != current.len() || current.len() != width * height * 3 {
        return Some((0, 0, width as u16 - 1, height as u16 - 1));
    }
    let tiles_x = width.div_ceil(tile);
    let tiles_y = height.div_ceil(tile);
    let mut min_x = usize::MAX;
    let mut min_y = usize::MAX;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    let mut changed_tiles = 0usize;
    for tile_y in 0..tiles_y {
        for tile_x in 0..tiles_x {
            let start_x = tile_x * tile;
            let start_y = tile_y * tile;
            let end_x = (start_x + tile).min(width);
            let end_y = (start_y + tile).min(height);
            let mut changed = false;
            'scan: for y in start_y..end_y {
                let row = y * width * 3;
                for x in start_x..end_x {
                    let offset = row + x * 3;
                    if previous[offset..offset + 3] != current[offset..offset + 3] {
                        changed = true;
                        break 'scan;
                    }
                }
            }
            if changed {
                changed_tiles += 1;
                min_x = min_x.min(start_x);
                min_y = min_y.min(start_y);
                max_x = max_x.max(end_x - 1);
                max_y = max_y.max(end_y - 1);
            }
        }
    }
    if changed_tiles == 0 {
        return None;
    }
    // 화면의 60% 이상이 바뀌면 부분 전송 대신 전체 프레임을 보낸다.
    if changed_tiles * 10 >= tiles_x * tiles_y * 6 {
        return Some((0, 0, width as u16 - 1, height as u16 - 1));
    }
    Some((min_x as u16, min_y as u16, max_x as u16, max_y as u16))
}

// ---------------------------------------------------------------- PNG (store-only, 무의존)

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            if crc & 1 == 1 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    crc ^ 0xFFFF_FFFF
}

fn png_chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 12);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    let mut crc_input = Vec::with_capacity(4 + payload.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(payload);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    out
}

/// RGB 프레임을 PNG로 저장한다(무압축 deflate, 외부 의존성 없음).
pub fn write_png(path: &std::path::Path, width: usize, height: usize, rgb: &[u8]) -> std::io::Result<()> {
    let mut raw = Vec::with_capacity(height * (1 + width * 3));
    for y in 0..height {
        raw.push(0); // filter: none
        let row = y * width * 3;
        raw.extend_from_slice(&rgb[row..row + width * 3]);
    }
    // zlib: header + stored blocks + adler32
    let mut zlib = vec![0x78, 0x01];
    let mut offset = 0usize;
    while offset < raw.len() {
        let take = (raw.len() - offset).min(65_535);
        let last = offset + take >= raw.len();
        zlib.push(if last { 1 } else { 0 });
        zlib.extend_from_slice(&(take as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(take as u16)).to_le_bytes());
        zlib.extend_from_slice(&raw[offset..offset + take]);
        offset += take;
    }
    let mut a = 1u32;
    let mut b = 0u32;
    for byte in &raw {
        a = (a + *byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8bit, truecolor, no interlace
    png.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&png_chunk(b"IDAT", &zlib));
    png.extend_from_slice(&png_chunk(b"IEND", &[]));
    std::fs::write(path, png)
}

/// 표시용 문자열 자르기(가로 스크롤 방지).
pub fn truncate(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = chars.into_iter().take(keep).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_rev_a_command_packets() {
        // 전체 화면 비트맵: x=0,y=0,ex=319,ey=479
        let bitmap = command_packet(CMD_DISPLAY_BITMAP, 0, 0, 319, 479);
        assert_eq!(bitmap, [0x00, 0x00, 0x04, 0xFD, 0xDF, CMD_DISPLAY_BITMAP]);
        // 단일 픽셀 x==ex, y==ey
        let pixel = command_packet(CMD_DISPLAY_BITMAP, 1, 2, 1, 2);
        assert_eq!(pixel[0], 0x00);
        assert_eq!(pixel[1], ((1 & 3) << 6) | (2 >> 4));
        assert_eq!(pixel[4], 2);
        assert_eq!(pixel[5], CMD_DISPLAY_BITMAP);
        // 최대 좌표(10비트) 왕복 복원
        let max = command_packet(0, 95, 0, 0, 0);
        assert_eq!(max[0], (95 >> 2) as u8);
        assert_eq!(max[1] >> 6, (95 & 3) as u8);
    }

    #[test]
    fn sends_hello_as_six_0x45() {
        assert_eq!(hello_packet(), [0x45; 6]);
    }

    #[test]
    fn parses_hello_models() {
        assert_eq!(parse_hello(&[1; 6]), PanelModel::UsbMonitor35);
        assert_eq!(parse_hello(&[2; 6]), PanelModel::UsbMonitor5);
        assert_eq!(parse_hello(&[3; 6]), PanelModel::UsbMonitor7);
        // 정식 Turing 3.5는 무응답이 정상
        assert_eq!(parse_hello(&[]), PanelModel::Turing35);
        assert_eq!(parse_hello(&[1, 2, 3, 4, 5, 6]), PanelModel::Turing35);
        assert_eq!(PanelModel::Turing35.size(), (320, 480));
        assert!(PanelModel::Turing35.is_supported());
        assert!(!PanelModel::UsbMonitor7.is_supported());
    }

    #[test]
    fn builds_orientation_packet() {
        let packet = orientation_packet(Orientation::Portrait);
        assert_eq!(&packet[..6], &command_packet(CMD_SET_ORIENTATION, 0, 0, 0, 0));
        assert_eq!(packet[6], 100);
        assert_eq!(&packet[7..11], &[0x01, 0x40, 0x01, 0xE0]);
        let landscape = orientation_packet(Orientation::Landscape);
        assert_eq!(landscape[6], 102);
        assert_eq!(&landscape[7..11], &[0x01, 0xE0, 0x01, 0x40]);
        assert_eq!(orientation_packet(Orientation::ReversePortrait)[6], 101);
        assert_eq!(orientation_packet(Orientation::ReverseLandscape)[6], 103);
    }

    #[test]
    fn inverts_brightness_percent() {
        assert_eq!(brightness_value(100), 0);
        assert_eq!(brightness_value(0), 255);
        assert_eq!(brightness_value(50), 128);
        assert_eq!(brightness_value(200), 0);
        let packet = brightness_packet(50);
        assert_eq!(packet[5], CMD_SET_BRIGHTNESS);
        assert_eq!(packet[0], (128 >> 2) as u8);
        assert_eq!(packet[1] >> 6, (128 & 3) as u8);
        assert_eq!(screen_packet(true)[5], CMD_SCREEN_ON);
        assert_eq!(screen_packet(false)[5], CMD_SCREEN_OFF);
    }

    #[test]
    fn orientation_transforms_and_sizes() {
        assert_eq!(Orientation::parse("landscape"), Orientation::Landscape);
        assert_eq!(Orientation::parse("Portrait"), Orientation::Portrait);
        assert_eq!(Orientation::from_rotation(0, Orientation::Portrait).size(), (320, 480));
        assert_eq!(Orientation::from_rotation(90, Orientation::Portrait).size(), (480, 320));
        assert_eq!(Orientation::from_rotation(180, Orientation::Portrait).hardware_value(), 1);
        assert_eq!(Orientation::from_rotation(270, Orientation::Portrait).hardware_value(), 3);
        assert_eq!(Orientation::Portrait.hardware_value(), 0);
        assert_eq!(Orientation::Landscape.hardware_value(), 2);
    }

    #[test]
    fn rotates_180_in_software() {
        let rgb = [
            1, 2, 3, 4, 5, 6, //
            7, 8, 9, 10, 11, 12,
        ];
        let rotated = rotate180(&rgb, 2, 2);
        assert_eq!(&rotated[0..3], &[10, 11, 12]);
        assert_eq!(&rotated[9..12], &[1, 2, 3]);
    }

    #[test]
    fn converts_rgb565_little_endian() {
        let rgb = [255, 0, 0, 0, 255, 0];
        let converted = rgb565_le(&rgb);
        assert_eq!(converted, vec![0x00, 0xF8, 0xE0, 0x07]);
    }

    #[test]
    fn detects_dirty_regions() {
        let width = 8;
        let height = 8;
        let blank = vec![0u8; width * height * 3];
        assert_eq!(dirty_rect(&blank, &blank, width, height, 4), None);

        let mut changed = blank.clone();
        let offset = (2 * width + 5) * 3;
        changed[offset] = 255;
        let rect = dirty_rect(&blank, &changed, width, height, 4).unwrap();
        assert_eq!(rect, (4, 0, 7, 3));

        // 크기 불일치는 전체 프레임
        assert_eq!(
            dirty_rect(&blank, &[], width, height, 4),
            Some((0, 0, 7, 7))
        );
    }

    #[test]
    fn writes_valid_png_header() {
        let dir = std::env::temp_dir().join("turzx-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("frame.png");
        let rgb = vec![0u8; 4 * 4 * 3];
        write_png(&path, 4, 4, &rgb).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert!(bytes.len() > 50);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncates_long_labels() {
        assert_eq!(truncate("qwen3.5:9b", 20), "qwen3.5:9b");
        assert_eq!(truncate("deepseek-r1:70b-q4_k_m", 14), "deepseek-r1:7…");
        assert_eq!(truncate("짧은이름", 20), "짧은이름");
    }

    #[test]
    fn chunk_size_matches_reference() {
        assert_eq!(chunk_size(320), 2560);
        assert_eq!(chunk_size(480), 3840);
    }
}
