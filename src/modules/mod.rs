pub mod calendar;
pub mod kchat;
pub mod kdrive;
pub mod mail;

use crate::{App, Error, Result, oauth::Actor};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

pub struct Call {
    pub app: App,
    pub actor: Actor,
    pub token: String,
    pub args: Value,
    pub mutation_context: Arc<Mutex<Option<Value>>>,
}
impl Call {
    /// Recovery metadata only: object IDs, phase and progress, never content or credentials.
    pub fn set_mutation_context(&self, context: Value) -> Result<()> {
        *self
            .mutation_context
            .lock()
            .map_err(|_| Error::internal())? = Some(context);
        Ok(())
    }
}
pub type Handler = fn(Call, String) -> Pin<Box<dyn Future<Output = Result<Value>> + Send>>;
pub struct Resources {
    pub scheme: &'static str,
    pub handler: Handler,
}
pub struct HttpRoute {
    pub path: &'static str,
    pub write: bool,
    pub handler: axum::routing::MethodRouter<App>,
}
/// Credential provider shared by all Infomaniak services: one token per user.
pub const INFOMANIAK: &str = "infomaniak";
pub struct Module {
    pub id: &'static str,
    pub name: &'static str,
    pub namespace: &'static str,
    /// Modules with the same provider share one stored credential per user.
    pub provider: &'static str,
    pub scopes: Vec<&'static str>,
    pub tools: Vec<Tool>,
    pub routes: Vec<HttpRoute>,
    pub resources: Option<Resources>,
    pub handler: Handler,
}
#[derive(Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub write: bool,
    pub destructive: bool,
    pub external: bool,
    pub schema: Value,
    pub output: Value,
}
impl Tool {
    pub fn metadata(&self, module: &str) -> Value {
        let scope = format!("{module}:{}", if self.write { "write" } else { "read" });
        json!({"name":self.name,"description":self.description,"inputSchema":self.schema,
            "outputSchema":{"type":"object","properties":{"data":self.output,"context":{"type":"object"},"has_more":{"type":"boolean"},"cursor":{"type":["string","null"]},"next_offset":{"type":["integer","null"]},"content":{"type":"array"}},"required":["data","context"]},
            "annotations":{"readOnlyHint":!self.write,"destructiveHint":self.destructive,"idempotentHint":!self.write,"openWorldHint":self.external},
            "securitySchemes":[{"type":"oauth2","scopes":[scope]}],"_meta":{"ui":{"resourceUri":"ui://ensemble/object.html"},"openai/outputTemplate":"ui://ensemble/object.html"}})
    }
    pub fn validate(&self, args: &Value) -> Result<()> {
        validate(args, &self.schema)
    }
}
pub struct Registry {
    pub modules: Vec<Module>,
}
impl Registry {
    pub fn builtin() -> Result<Self> {
        Self::new(vec![
            mail::module(),
            calendar::module(),
            kdrive::module(),
            kchat::module(),
        ])
    }
    pub fn new(modules: Vec<Module>) -> Result<Self> {
        let (mut ids, mut namespaces, mut routes, mut tools) = (
            HashSet::new(),
            HashSet::new(),
            HashSet::new(),
            HashSet::new(),
        );
        let mut schemes = HashSet::new();
        for m in &modules {
            if m.id.is_empty()
                || m.namespace.is_empty()
                || m.provider.is_empty()
                || !ids.insert(m.id)
                || !namespaces.insert(m.namespace)
                || m.routes.iter().any(|r| {
                    !r.path.starts_with(&format!("/modules/{}/", m.id)) || !routes.insert(r.path)
                })
                || m.resources
                    .as_ref()
                    .is_some_and(|r| r.scheme.is_empty() || !schemes.insert(r.scheme))
                || m.tools.iter().any(|t| {
                    !t.name.starts_with(&format!("{}_", m.namespace))
                        || !tools.insert(t.name.clone())
                })
            {
                return Err(Error::config(
                    "Conflicting module ID, namespace, route or tool",
                ));
            }
        }
        Ok(Self { modules })
    }
    pub fn get(&self, id: &str) -> Result<&Module> {
        self.modules
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| Error::invalid("Unknown module"))
    }
    pub fn tool(&self, name: &str) -> Result<(&Module, &Tool)> {
        self.modules
            .iter()
            .find_map(|m| m.tools.iter().find(|t| t.name == name).map(|t| (m, t)))
            .ok_or_else(|| Error::invalid("Unknown tool"))
    }
}

pub fn s() -> Value {
    json!({"type":"string","maxLength":100000})
}
pub fn id() -> Value {
    json!({"type":"string","minLength":1,"maxLength":256})
}
pub fn path_id() -> Value {
    json!({"type":"string","minLength":1,"maxLength":256,"format":"provider-id","description":"Opaque provider identifier; path delimiters and dot segments are not permitted"})
}
pub fn safe_provider_id(raw: &str) -> bool {
    !raw.is_empty()
        && raw != "."
        && raw != ".."
        && !raw.chars().any(|c| c.is_control() || "/\\%".contains(c))
}
pub fn n() -> Value {
    json!({"type":"integer","minimum":1,"maximum":9007199254740991_i64})
}
pub fn b() -> Value {
    json!({"type":"boolean"})
}
pub fn choices(values: &[&str]) -> Value {
    json!({"type":"string","enum":values})
}
pub fn arr(item: Value) -> Value {
    json!({"type":"array","items":item,"minItems":1,"maxItems":100})
}
pub fn tool(
    name: &str,
    description: &str,
    write: bool,
    destructive: bool,
    external: bool,
    required: &[&str],
    fields: Vec<(&str, Value)>,
) -> Tool {
    Tool {
        name: name.into(),
        description: description.into(),
        write,
        destructive,
        external,
        schema: json!({"type":"object","properties":fields.into_iter().map(|(k,v)|(k.into(),v)).collect::<serde_json::Map<_,_>>(),"required":required,"additionalProperties":false}),
        output: json!({"type":"object"}),
    }
}
fn validate(value: &Value, schema: &Value) -> Result<()> {
    let valid = match schema["type"].as_str().unwrap_or("") {
        "object" => value.is_object(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        _ => false,
    };
    if !valid {
        return Err(Error::invalid("Argument has the wrong type"));
    }
    if let Some(values) = schema["enum"].as_array()
        && !values.contains(value)
    {
        return Err(Error::invalid("Argument is outside the permitted values"));
    }
    if let Some(text) = value.as_str() {
        if schema["format"] == "provider-id" && !safe_provider_id(text) {
            return Err(Error::invalid("Invalid provider identifier"));
        }
        let len = text.chars().count() as u64;
        if schema["minLength"].as_u64().is_some_and(|n| len < n)
            || schema["maxLength"].as_u64().is_some_and(|n| len > n)
        {
            return Err(Error::invalid("String argument length is out of bounds"));
        }
    }
    if let Some(number) = value.as_i64()
        && (schema["minimum"].as_i64().is_some_and(|n| number < n)
            || schema["maximum"].as_i64().is_some_and(|n| number > n))
    {
        return Err(Error::invalid("Numeric argument is out of bounds"));
    }
    if let Some(fields) = value.as_object() {
        let props = schema["properties"]
            .as_object()
            .ok_or_else(Error::internal)?;
        if schema["required"].as_array().is_some_and(|required| {
            required
                .iter()
                .any(|k| !fields.contains_key(k.as_str().unwrap_or("")))
        }) {
            return Err(Error::invalid("Missing required argument"));
        }
        for (key, v) in fields {
            let def = props
                .get(key)
                .ok_or_else(|| Error::invalid("Unknown argument"))?;
            validate(v, def)?;
        }
    }
    if let Some(items) = value.as_array() {
        if schema["minItems"]
            .as_u64()
            .is_some_and(|n| items.len() < (n as usize))
            || schema["maxItems"]
                .as_u64()
                .is_some_and(|n| items.len() > (n as usize))
        {
            return Err(Error::invalid("Array length is out of bounds"));
        }
        for item in items {
            validate(item, &schema["items"])?;
        }
    }
    Ok(())
}
pub fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .ok_or_else(|| Error::invalid("Missing string argument"))
}
pub fn numeric(args: &Value, key: &str, default: i64) -> i64 {
    args[key].as_i64().unwrap_or(default)
}
pub fn segment(raw: &str) -> String {
    url::form_urlencoded::byte_serialize(raw.as_bytes()).collect::<String>()
}
pub fn data(value: Value) -> Value {
    value.get("data").cloned().unwrap_or(value)
}
pub fn provider_object(value: Value, mutation: bool) -> Result<Value> {
    let value = data(value);
    if !value.is_object() {
        return Err(Error::new(
            502,
            if mutation {
                "mutation_uncertain"
            } else {
                "provider_error"
            },
            "Invalid provider object; verify any mutation outcome before repeating",
        ));
    }
    Ok(value)
}
pub fn result(data: Value, context: Value) -> Value {
    json!({"data":if data.is_object() || data.is_array(){data}else{json!({"provider_result":data})},"context":context})
}
pub fn email(raw: &str) -> bool {
    raw.len() <= 320
        && raw.split('@').count() == 2
        && raw.split('@').all(|s| !s.is_empty())
        && raw.contains('.')
        && !raw
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "<>,;".contains(c))
}
