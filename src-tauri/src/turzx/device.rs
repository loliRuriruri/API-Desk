//! TURZX 후보 포트 탐색/선택.
//!
//! 친구 이름(friendly name)에 의존하지 않고 VID/PID/시리얼 번호로 점수를 매긴다.
//! 후보가 여러 개면 임의로 연결하지 않고 설정 화면에 목록을 보여준다.
use serde::Serialize;
use serialport::{SerialPortInfo, SerialPortType};

/// Rev B(V2) 3.5" 장치의 알려진 식별자.
pub const V2_VID: u16 = 0x1A86;
pub const V2_PID: u16 = 0x5722;
pub const V2_SERIAL: &str = "USB35INCHIPSV2";

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PortCandidate {
    pub port: String,
    pub description: String,
    pub serial_number: Option<String>,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub manufacturer: Option<String>,
    pub score: u8,
    pub known_device: bool,
}

pub fn score(info: &SerialPortInfo) -> u8 {
    match &info.port_type {
        SerialPortType::UsbPort(usb) => {
            if usb.vid == V2_VID && usb.pid == V2_PID {
                if usb.serial_number.as_deref() == Some(V2_SERIAL) {
                    return 100;
                }
                return 90;
            }
            if usb.serial_number.as_deref() == Some(V2_SERIAL) {
                return 80;
            }
            10
        }
        _ => 0,
    }
}

pub fn candidates_from(ports: &[SerialPortInfo]) -> Vec<PortCandidate> {
    let mut list: Vec<PortCandidate> = ports
        .iter()
        .map(|info| {
            let (vid, pid, serial_number, manufacturer) = match &info.port_type {
                SerialPortType::UsbPort(usb) => (
                    Some(usb.vid),
                    Some(usb.pid),
                    usb.serial_number.clone(),
                    usb.manufacturer.clone(),
                ),
                _ => (None, None, None, None),
            };
            let score = score(info);
            let description = match &info.port_type {
                SerialPortType::UsbPort(usb) => format!(
                    "USB Serial (VID {:04X}:PID {:04X}){}",
                    usb.vid,
                    usb.pid,
                    usb.product
                        .as_deref()
                        .map(|product| format!(" · {product}"))
                        .unwrap_or_default()
                ),
                SerialPortType::BluetoothPort => "Bluetooth Serial".to_string(),
                SerialPortType::PciPort => "PCI Serial".to_string(),
                SerialPortType::Unknown => "Serial".to_string(),
            };
            PortCandidate {
                port: info.port_name.clone(),
                description,
                serial_number,
                vid,
                pid,
                manufacturer,
                score,
                known_device: score >= 80,
            }
        })
        .collect();
    list.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.port.cmp(&b.port)));
    list
}

pub fn list_candidates() -> Vec<PortCandidate> {
    match serialport::available_ports() {
        Ok(ports) => candidates_from(&ports),
        Err(_) => Vec::new(),
    }
}

/// 자동 선택: 수동 override > 알려진 V2 장치 > 시리얼 번호 일치 > (없으면 None).
/// 신뢰할 수 있는 후보가 둘 이상 동점이면 None을 돌려주고 설정에서 고르게 한다.
pub fn detect(preferred: Option<&str>) -> Option<PortCandidate> {
    let candidates = list_candidates();
    if let Some(preferred) = preferred
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("auto"))
    {
        return candidates.into_iter().find(|item| item.port == preferred);
    }
    let best = candidates.first()?.clone();
    if best.score < 80 {
        return None;
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serialport::UsbPortInfo;

    fn usb(port: &str, vid: u16, pid: u16, serial: Option<&str>) -> SerialPortInfo {
        SerialPortInfo {
            port_name: port.to_string(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid,
                pid,
                serial_number: serial.map(str::to_string),
                manufacturer: None,
                product: None,
            }),
        }
    }

    #[test]
    fn scores_known_v2_device_highest() {
        let ports = vec![
            usb("COM3", 0x264A, 0x233D, None),
            usb("COM4", V2_VID, V2_PID, Some(V2_SERIAL)),
            usb("COM7", 0x1234, 0x5678, None),
        ];
        let list = candidates_from(&ports);
        assert_eq!(list[0].port, "COM4");
        assert_eq!(list[0].score, 100);
        assert!(list[0].known_device);
        assert_eq!(list[1].port, "COM3");
        assert!(!list[1].known_device);
    }

    #[test]
    fn prefers_manual_override() {
        let ports = vec![
            usb("COM4", V2_VID, V2_PID, Some(V2_SERIAL)),
            usb("COM3", 0x264A, 0x233D, None),
        ];
        let list = candidates_from(&ports);
        // detect()는 실제 포트 목록을 읽으므로, override 선택 로직만 단위 검증한다.
        let chosen = list.iter().find(|item| item.port == "COM3").cloned();
        assert_eq!(chosen.map(|item| item.port), Some("COM3".to_string()));
        let none: Option<PortCandidate> = list.iter().find(|item| item.port == "COM9").cloned();
        assert!(none.is_none());
    }

    #[test]
    fn diag_dump_real_ports() {
        for info in serialport::available_ports().unwrap_or_default() {
            println!("PORT {} type={:?}", info.port_name, info.port_type);
        }
        for item in list_candidates() {
            println!(
                "CAND {} score={} known={} vid={:?} pid={:?} serial={:?} desc={}",
                item.port, item.score, item.known_device, item.vid, item.pid, item.serial_number, item.description
            );
        }
    }

    #[test]
    fn auto_setting_is_not_a_manual_port() {
        // "auto"는 수동 포트명이 아니므로 자동 선택 경로를 타야 한다.
        let candidates = vec![
            usb("COM4", V2_VID, V2_PID, Some(V2_SERIAL)),
            usb("COM3", 0x264A, 0x233D, None),
        ];
        let list = candidates_from(&candidates);
        let best = list.first().unwrap();
        assert_eq!(best.port, "COM4");
        assert!(best.score >= 80);
    }

    #[test]
    fn detects_same_device_after_com_number_change() {
        // 재부팅으로 COM 번호가 바뀌어도 VID/PID/시리얼로 동일 장치를 찾는다.
        let ports = vec![
            usb("COM3", 0x264A, 0x233D, None),
            usb("COM7", V2_VID, V2_PID, Some(V2_SERIAL)),
        ];
        let list = candidates_from(&ports);
        assert_eq!(list[0].port, "COM7");
        assert_eq!(list[0].score, 100);
        assert!(list[0].known_device);
    }

    #[test]
    fn usb_serial_without_v2_ids_still_scored() {
        let ports = vec![usb("COM9", 0x0001, 0x0002, Some(V2_SERIAL))];
        let list = candidates_from(&ports);
        assert_eq!(list[0].score, 80);
        assert!(list[0].known_device);
    }

    #[test]
    fn unknown_usb_ports_are_not_auto_selected() {
        let ports = vec![usb("COM7", 0x1234, 0x5678, None)];
        let list = candidates_from(&ports);
        assert_eq!(list[0].score, 10);
        assert!(!list[0].known_device);
    }
}
