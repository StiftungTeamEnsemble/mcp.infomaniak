//! Portable MCP Apps resources; each module owns its view and tool association.
//! https://developers.openai.com/plugins/build/chatgpt-ui
use serde_json::{Value, json};

pub const MIME: &str = "text/html;profile=mcp-app";

#[derive(Clone)]
pub struct Ui {
    pub uri: &'static str,
    pub title: &'static str,
    pub script: &'static str,
}
impl Ui {
    pub fn metadata(&self) -> Value {
        json!({"uri":self.uri,"name":self.title,"mimeType":MIME})
    }
    pub fn read(&self) -> Value {
        // Only compile-time assets enter this document. Provider content arrives
        // through the host bridge and is never interpolated into executable HTML.
        let html = format!(
            "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>{}</style><body><main id=\"app\" aria-live=\"polite\"><p>Loading…</p></main><script>{}\n{}</script></body></html>",
            self.title,
            include_str!("mcp_apps/style.css"),
            include_str!("mcp_apps/bridge.js"),
            self.script,
        );
        json!({"contents":[{"uri":self.uri,"mimeType":MIME,"text":html,"_meta":{"ui":{"prefersBorder":true,"csp":{"connectDomains":[],"resourceDomains":[],"frameDomains":[],"baseUriDomains":[]}}}}]})
    }
}
