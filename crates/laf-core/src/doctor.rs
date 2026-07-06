//! Environment "doctor" report types. Each platform crate fills one of these
//! in; the UI renders it in onboarding/settings and `local-ai-flow --doctor`
//! prints it to the terminal with exact fix instructions.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    /// Works with reduced capability (a fallback will be used).
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    /// Stable id, e.g. "wayland.ydotool".
    pub id: String,
    pub label: String,
    pub status: CheckStatus,
    pub detail: String,
    /// Exact, copy-pasteable remediation (shell command or Settings pane).
    pub fix_hint: String,
}

impl DoctorCheck {
    pub fn ok(id: &str, label: &str, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status: CheckStatus::Ok,
            detail: detail.into(),
            fix_hint: String::new(),
        }
    }
    pub fn warn(id: &str, label: &str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status: CheckStatus::Warn,
            detail: detail.into(),
            fix_hint: fix.into(),
        }
    }
    pub fn fail(id: &str, label: &str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status: CheckStatus::Fail,
            detail: detail.into(),
            fix_hint: fix.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    /// "macos" | "linux".
    pub platform: String,
    /// Linux: "wayland" | "x11" | "unknown"; macOS: OS version.
    pub session: String,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub fn worst(&self) -> CheckStatus {
        self.checks.iter().fold(CheckStatus::Ok, |acc, c| match (acc, c.status) {
            (_, CheckStatus::Fail) | (CheckStatus::Fail, _) => CheckStatus::Fail,
            (_, CheckStatus::Warn) | (CheckStatus::Warn, _) => CheckStatus::Warn,
            _ => CheckStatus::Ok,
        })
    }

    /// Terminal rendering for `--doctor`.
    pub fn to_terminal(&self) -> String {
        let mut out = format!(
            "Local AI Flow doctor — platform: {}, session: {}\n\n",
            self.platform, self.session
        );
        for c in &self.checks {
            let mark = match c.status {
                CheckStatus::Ok => "[ ok ]",
                CheckStatus::Warn => "[warn]",
                CheckStatus::Fail => "[FAIL]",
            };
            out.push_str(&format!("{mark} {} — {}\n", c.label, c.detail));
            if c.status != CheckStatus::Ok && !c.fix_hint.is_empty() {
                out.push_str(&format!("       fix: {}\n", c.fix_hint));
            }
        }
        out
    }
}
