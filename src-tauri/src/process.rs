//! 프로세스 실행 유틸리티.
//!
//! Windows 상주/백그라운드 실행 중 자식 콘솔 프로세스(PowerShell, Grok, netstat 등)가
//! 화면에 깜빡이는 현상(Console Flash)을 방지하기 위해 `CREATE_NO_WINDOW` 플래그를 중앙 집중 적용한다.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// Windows Win32 `CREATE_NO_WINDOW` 플래그 (0x08000000).
/// 콘솔 창 생성 없이 프로세스를 실행한다.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 백그라운드 콘솔 프로세스 생성 헬퍼.
/// Windows에서는 `CREATE_NO_WINDOW` 플래그를 자동으로 주입하고,
/// 진단 로그(resident.log)에 실행 파일명만 기록한다(시크릿/인자 제외).
pub fn hidden_command<S: AsRef<OsStr>>(program: S) -> Command {
    let mut command = Command::new(&program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let exe_name = Path::new(program.as_ref())
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");
    crate::resident::log_child_process(exe_name);
    command
}

/// 기존 `Command` 객체에 `CREATE_NO_WINDOW` 플래그를 설정하는 확장 트레이트.
pub trait HiddenCommandExt {
    fn creation_flags_hidden(&mut self) -> &mut Self;
}

impl HiddenCommandExt for Command {
    fn creation_flags_hidden(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_clean_executable_name() {
        let name = Path::new(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe")
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap();
        assert_eq!(name, "powershell.exe");
    }

    #[cfg(windows)]
    #[test]
    fn hidden_command_runs_without_error() {
        let output = hidden_command("cmd.exe")
            .args(["/c", "echo", "hidden_test_ok"])
            .output()
            .expect("cmd execution failed");
        assert!(output.status.success());
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("hidden_test_ok"));
    }

    #[cfg(windows)]
    #[test]
    fn hidden_command_captures_exit_code_and_stderr() {
        let output = hidden_command("cmd.exe")
            .args(["/c", "echo err_msg 1>&2 & exit 7"])
            .output()
            .expect("cmd execution failed");
        assert_eq!(output.status.code(), Some(7));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("err_msg"));
    }

    #[cfg(windows)]
    #[test]
    fn hidden_powershell_runs_non_interactive() {
        let output = hidden_command("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                "Write-Output 'ps_non_interactive_ok'",
            ])
            .output()
            .expect("powershell execution failed");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("ps_non_interactive_ok"));
    }
}
