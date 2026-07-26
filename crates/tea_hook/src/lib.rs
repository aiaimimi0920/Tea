#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use tea_core::{ActorRef, TicketSource};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookIntakeRequest {
    pub source: String,
    pub text: String,
    pub context: HookContext,
    #[serde(default)]
    pub attachments: Vec<HookAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookContext {
    pub active_window: Option<String>,
    pub selection_text: Option<String>,
    pub ocr_text: Option<String>,
    pub screenshot_ref: Option<String>,
    pub cwd: Option<String>,
    pub app: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookAttachment {
    pub kind: String,
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedHookTicket {
    pub title: String,
    pub description: String,
    pub source: TicketSource,
    pub actor: ActorRef,
    pub labels: Vec<String>,
}

pub fn normalize_hook_intake(request: &HookIntakeRequest) -> NormalizedHookTicket {
    let title = first_chars_or_fallback(&request.text, 80, "Hook ticket");
    let mut description = request.text.clone();
    description.push_str("\n\n--- Hook context (untrusted) ---\n");
    append_context(
        &mut description,
        "active_window",
        &request.context.active_window,
    );
    append_context(
        &mut description,
        "selection_text",
        &request.context.selection_text,
    );
    append_context(&mut description, "ocr_text", &request.context.ocr_text);
    append_context(
        &mut description,
        "screenshot_ref",
        &request.context.screenshot_ref,
    );
    append_context(&mut description, "cwd", &request.context.cwd);
    append_context(&mut description, "app", &request.context.app);

    NormalizedHookTicket {
        title,
        description,
        source: TicketSource::Hook,
        actor: ActorRef::hook(if request.source.is_empty() {
            "hook"
        } else {
            request.source.as_str()
        }),
        labels: vec![
            "source:hook".to_string(),
            "policy:plan-only".to_string(),
            "context:untrusted".to_string(),
        ],
    }
}

fn append_context(description: &mut String, key: &str, value: &Option<String>) {
    if let Some(value) = value {
        description.push_str(key);
        description.push_str(": ");
        description.push_str(value);
        description.push('\n');
    }
}

fn first_chars_or_fallback(value: &str, max_chars: usize, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    trimmed.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_intake_normalizes_to_plan_only_untrusted_ticket() {
        let normalized = normalize_hook_intake(&HookIntakeRequest {
            source: "desktop".to_string(),
            text: "Please analyze current failure".to_string(),
            context: HookContext {
                active_window: Some("PowerShell".to_string()),
                selection_text: Some("cargo test failed".to_string()),
                ocr_text: None,
                screenshot_ref: None,
                cwd: Some("C:\\repo".to_string()),
                app: Some("terminal".to_string()),
            },
            attachments: vec![],
        });

        assert_eq!(normalized.source, TicketSource::Hook);
        assert!(normalized.labels.contains(&"context:untrusted".to_string()));
        assert!(normalized.description.contains("cargo test failed"));
    }
}
