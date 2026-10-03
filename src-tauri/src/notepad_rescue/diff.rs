use serde::{Deserialize, Serialize};

/// 라인 diff 유형 (UNCHANGED, ADDED, REMOVED).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiffKind {
    Unchanged,
    Added,
    Removed,
}

/// 연속된 동일 diff 유형의 라인 블록.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffBlock {
    pub kind: DiffKind,
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub lines: Vec<String>,
}

/// 두 스냅샷 간의 라인 diff 비교 결과 DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffResultDto {
    pub note_id: String,
    pub base_snapshot_id: String,
    pub target_snapshot_id: String,
    pub is_too_large: bool,
    pub total_added_lines: usize,
    pub total_removed_lines: usize,
    pub total_unchanged_lines: usize,
    pub blocks: Vec<DiffBlock>,
    pub message: Option<String>,
}

const MAX_DIFF_BYTES: usize = 1_000_000; // 1 MB
const MAX_DIFF_LINES: usize = 10_000;
const MAX_DIFF_STEPS: usize = 2_000;

/// 텍스트를 줄 단위로 분할합니다 (CRLF 및 LF 정규화).
fn split_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    // \r\n 과 \n 을 균일하게 분할
    text.split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect()
}

/// 두 텍스트의 라인 단위 diff를 안전하게 계산합니다.
pub fn compare_texts(
    note_id: &str,
    base_snapshot_id: &str,
    target_snapshot_id: &str,
    old_text: &str,
    new_text: &str,
) -> DiffResultDto {
    let lines_a = split_lines(old_text);
    let lines_b = split_lines(new_text);

    let is_oversized = old_text.len() > MAX_DIFF_BYTES
        || new_text.len() > MAX_DIFF_BYTES
        || lines_a.len() > MAX_DIFF_LINES
        || lines_b.len() > MAX_DIFF_LINES;

    if is_oversized {
        return DiffResultDto {
            note_id: note_id.to_string(),
            base_snapshot_id: base_snapshot_id.to_string(),
            target_snapshot_id: target_snapshot_id.to_string(),
            is_too_large: true,
            total_added_lines: lines_b.len(),
            total_removed_lines: lines_a.len(),
            total_unchanged_lines: 0,
            blocks: Vec::new(),
            message: Some(format!(
                "DIFF_TOO_LARGE: 비교 대상 문서가 너무 커서(최대 1MB / 10,000줄 초과) 상세 줄별 diff가 비활성화되었습니다. (기존: {}줄, 대상: {}줄)",
                lines_a.len(),
                lines_b.len()
            )),
        };
    }

    // 1. 공통 접두사(prefix) 추출
    let mut prefix_len = 0;
    while prefix_len < lines_a.len()
        && prefix_len < lines_b.len()
        && lines_a[prefix_len] == lines_b[prefix_len]
    {
        prefix_len += 1;
    }

    // 2. 공통 접미사(suffix) 추출
    let mut suffix_len = 0;
    while suffix_len + prefix_len < lines_a.len()
        && suffix_len + prefix_len < lines_b.len()
        && lines_a[lines_a.len() - 1 - suffix_len] == lines_b[lines_b.len() - 1 - suffix_len]
    {
        suffix_len += 1;
    }

    let mid_a = &lines_a[prefix_len..(lines_a.len() - suffix_len)];
    let mid_b = &lines_b[prefix_len..(lines_b.len() - suffix_len)];

    // 3. Myers Diff를 중간 변경 영역에 적용
    let mid_ops = if mid_a.is_empty() && mid_b.is_empty() {
        Vec::new()
    } else if mid_a.is_empty() {
        mid_b
            .iter()
            .map(|l| (DiffKind::Added, l.clone()))
            .collect::<Vec<_>>()
    } else if mid_b.is_empty() {
        mid_a
            .iter()
            .map(|l| (DiffKind::Removed, l.clone()))
            .collect::<Vec<_>>()
    } else if mid_a.len() + mid_b.len() > MAX_DIFF_STEPS {
        // 복잡도 제한 초과 시 단순 대체로 처리
        let mut ops = Vec::with_capacity(mid_a.len() + mid_b.len());
        for l in mid_a {
            ops.push((DiffKind::Removed, l.clone()));
        }
        for l in mid_b {
            ops.push((DiffKind::Added, l.clone()));
        }
        ops
    } else {
        compute_myers_diff(mid_a, mid_b)
    };

    // 4. 접두사 + 중간 연산 + 접미사 통합
    let mut all_ops = Vec::with_capacity(prefix_len + mid_ops.len() + suffix_len);
    for l in &lines_a[..prefix_len] {
        all_ops.push((DiffKind::Unchanged, l.clone()));
    }
    all_ops.extend(mid_ops);
    for l in &lines_a[(lines_a.len() - suffix_len)..] {
        all_ops.push((DiffKind::Unchanged, l.clone()));
    }

    // 5. 연속된 연산을 DiffBlock 단위로 그룹화
    let mut blocks: Vec<DiffBlock> = Vec::new();
    let mut old_line_idx = 1;
    let mut new_line_idx = 1;
    let mut total_added = 0;
    let mut total_removed = 0;
    let mut total_unchanged = 0;

    for (kind, line) in all_ops {
        match kind {
            DiffKind::Unchanged => total_unchanged += 1,
            DiffKind::Added => total_added += 1,
            DiffKind::Removed => total_removed += 1,
        }

        let should_merge = match blocks.last() {
            Some(b) => b.kind == kind,
            None => false,
        };

        if should_merge {
            let last = blocks.last_mut().unwrap();
            last.lines.push(line);
            match kind {
                DiffKind::Unchanged => {
                    last.old_count += 1;
                    last.new_count += 1;
                    old_line_idx += 1;
                    new_line_idx += 1;
                }
                DiffKind::Added => {
                    last.new_count += 1;
                    new_line_idx += 1;
                }
                DiffKind::Removed => {
                    last.old_count += 1;
                    old_line_idx += 1;
                }
            }
        } else {
            let (old_start, old_count, new_start, new_count) = match kind {
                DiffKind::Unchanged => {
                    let os = old_line_idx;
                    let ns = new_line_idx;
                    old_line_idx += 1;
                    new_line_idx += 1;
                    (os, 1, ns, 1)
                }
                DiffKind::Added => {
                    let ns = new_line_idx;
                    new_line_idx += 1;
                    (old_line_idx, 0, ns, 1)
                }
                DiffKind::Removed => {
                    let os = old_line_idx;
                    old_line_idx += 1;
                    (os, 1, new_line_idx, 0)
                }
            };

            blocks.push(DiffBlock {
                kind,
                old_start,
                old_count,
                new_start,
                new_count,
                lines: vec![line],
            });
        }
    }

    DiffResultDto {
        note_id: note_id.to_string(),
        base_snapshot_id: base_snapshot_id.to_string(),
        target_snapshot_id: target_snapshot_id.to_string(),
        is_too_large: false,
        total_added_lines: total_added,
        total_removed_lines: total_removed,
        total_unchanged_lines: total_unchanged,
        blocks,
        message: None,
    }
}

/// Myers O(ND) 최단 편집 스크립트(SES) 알고리즘
fn compute_myers_diff(lines_a: &[String], lines_b: &[String]) -> Vec<(DiffKind, String)> {
    let n = lines_a.len();
    let m = lines_b.len();
    let max_d = n + m;
    if max_d == 0 {
        return Vec::new();
    }

    let offset = max_d as isize;
    let mut v = vec![0isize; 2 * max_d + 1];
    let mut trace: Vec<Vec<isize>> = Vec::new();

    'outer: for d in 0..=max_d {
        trace.push(v.clone());
        for k in (-(d as isize)..=(d as isize)).step_by(2) {
            let mut x = if k == -(d as isize)
                || (k != d as isize && v[(k - 1 + offset) as usize] < v[(k + 1 + offset) as usize])
            {
                v[(k + 1 + offset) as usize]
            } else {
                v[(k - 1 + offset) as usize] + 1
            };
            let mut y = x - k;

            while x < n as isize && y < m as isize && lines_a[x as usize] == lines_b[y as usize] {
                x += 1;
                y += 1;
            }
            v[(k + offset) as usize] = x;

            if x >= n as isize && y >= m as isize {
                break 'outer;
            }
        }
    }

    // 역추적(Backtracking)하여 변경 연산 시퀀스 생성
    let mut script = Vec::new();
    let mut x = n as isize;
    let mut y = m as isize;

    for d in (0..trace.len()).rev() {
        let v_prev = &trace[d];
        let k = x - y;
        let prev_k = if k == -(d as isize)
            || (d > 0
                && k != d as isize
                && v_prev[(k - 1 + offset) as usize] < v_prev[(k + 1 + offset) as usize])
        {
            k + 1
        } else {
            k - 1
        };

        let prev_x = v_prev[(prev_k + offset) as usize];
        let prev_y = prev_x - prev_k;

        while x > prev_x && y > prev_y {
            script.push((DiffKind::Unchanged, lines_a[(x - 1) as usize].clone()));
            x -= 1;
            y -= 1;
        }

        if d > 0 {
            if x == prev_x {
                script.push((DiffKind::Added, lines_b[(y - 1) as usize].clone()));
                y -= 1;
            } else if y == prev_y {
                script.push((DiffKind::Removed, lines_a[(x - 1) as usize].clone()));
                x -= 1;
            }
        }
    }

    script.reverse();
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_identical_versions() {
        let text = "Line 1\nLine 2\nLine 3";
        let res = compare_texts("n1", "s1", "s2", text, text);
        assert!(!res.is_too_large);
        assert_eq!(res.total_added_lines, 0);
        assert_eq!(res.total_removed_lines, 0);
        assert_eq!(res.total_unchanged_lines, 3);
        assert_eq!(res.blocks.len(), 1);
        assert_eq!(res.blocks[0].kind, DiffKind::Unchanged);
    }

    #[test]
    fn test_diff_line_addition() {
        let old = "Line 1\nLine 3";
        let new = "Line 1\nLine 2\nLine 3";
        let res = compare_texts("n1", "s1", "s2", old, new);
        assert_eq!(res.total_added_lines, 1);
        assert_eq!(res.total_removed_lines, 0);
        assert_eq!(res.total_unchanged_lines, 2);
    }

    #[test]
    fn test_diff_line_removal() {
        let old = "Line 1\nLine 2\nLine 3";
        let new = "Line 1\nLine 3";
        let res = compare_texts("n1", "s1", "s2", old, new);
        assert_eq!(res.total_added_lines, 0);
        assert_eq!(res.total_removed_lines, 1);
        assert_eq!(res.total_unchanged_lines, 2);
    }

    #[test]
    fn test_diff_line_replacement() {
        let old = "Alpha\nBeta\nGamma";
        let new = "Alpha\nDelta\nGamma";
        let res = compare_texts("n1", "s1", "s2", old, new);
        assert_eq!(res.total_added_lines, 1);
        assert_eq!(res.total_removed_lines, 1);
        assert_eq!(res.total_unchanged_lines, 2);
    }

    #[test]
    fn test_diff_multiline_edit() {
        let old = "A\nB\nC\nD\nE";
        let new = "A\nX\nY\nD\nZ";
        let res = compare_texts("n1", "s1", "s2", old, new);
        assert!(res.total_added_lines >= 2);
        assert!(res.total_removed_lines >= 2);
    }

    #[test]
    fn test_diff_korean() {
        let old = "첫 번째 줄\n두 번째 줄\n세 번째 줄";
        let new = "첫 번째 줄\n수정된 두 번째 줄\n세 번째 줄\n새로운 네 번째 줄";
        let res = compare_texts("n1", "s1", "s2", old, new);
        assert_eq!(res.total_added_lines, 2);
        assert_eq!(res.total_removed_lines, 1);
    }

    #[test]
    fn test_diff_japanese() {
        let old = "メモ帳のバックアップ\n設定を変更します";
        let new = "メモ帳のバックアップ\n設定を安全に更新しました";
        let res = compare_texts("n1", "s1", "s2", old, new);
        assert_eq!(res.total_added_lines, 1);
        assert_eq!(res.total_removed_lines, 1);
    }

    #[test]
    fn test_diff_emoji() {
        let old = "중요 문서 📝\n서버 정보 🖥️";
        let new = "중요 문서 📝\n클라우드 정보 ☁️\n서버 정보 🖥️ 🚀";
        let res = compare_texts("n1", "s1", "s2", old, new);
        assert!(res.total_added_lines >= 1);
    }

    #[test]
    fn test_diff_empty_to_content() {
        let old = "";
        let new = "새로 작성된 문서 1줄\n2줄";
        let res = compare_texts("n1", "s1", "s2", old, new);
        assert_eq!(res.total_added_lines, 2);
        assert_eq!(res.total_removed_lines, 0);
        assert_eq!(res.total_unchanged_lines, 0);
        assert_eq!(res.blocks.len(), 1);
        assert_eq!(res.blocks[0].kind, DiffKind::Added);
    }

    #[test]
    fn test_diff_content_to_empty() {
        let old = "기존 내용 1줄\n2줄";
        let new = "";
        let res = compare_texts("n1", "s1", "s2", old, new);
        assert_eq!(res.total_added_lines, 0);
        assert_eq!(res.total_removed_lines, 2);
        assert_eq!(res.total_unchanged_lines, 0);
        assert_eq!(res.blocks.len(), 1);
        assert_eq!(res.blocks[0].kind, DiffKind::Removed);
    }

    #[test]
    fn test_diff_large_diff_bounded_behavior() {
        let large_old = "A\n".repeat(12_000);
        let large_new = "B\n".repeat(12_000);
        let res = compare_texts("n1", "s1", "s2", &large_old, &large_new);
        assert!(res.is_too_large);
        assert!(res.blocks.is_empty());
        assert!(res.message.is_some());
    }
}
