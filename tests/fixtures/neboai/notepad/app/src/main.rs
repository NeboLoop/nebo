use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nebo_sdk::error::NeboError;
use nebo_sdk::schema::SchemaBuilder;
use nebo_sdk::tool::ToolHandler;
use nebo_sdk::ui::{HttpRequest, HttpResponse, UiHandler};
use nebo_sdk::NeboApp;
use serde::Deserialize;
use serde_json::{json, Value};

type Notes = Arc<Mutex<HashMap<String, String>>>;

struct NotepadTool {
    notes: Notes,
}

struct NotepadUi {
    notes: Notes,
}

#[derive(Deserialize)]
struct Input {
    action: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    content: String,
}

#[async_trait]
impl ToolHandler for NotepadTool {
    fn name(&self) -> &str {
        "notepad"
    }

    fn description(&self) -> &str {
        "Save, read, and list persistent text notes."
    }

    fn schema(&self) -> Value {
        SchemaBuilder::new(&["save", "read", "list"])
            .string("key", "Note identifier", false)
            .string("content", "Text content to save", false)
            .build()
    }

    async fn execute(&self, input: Value) -> Result<String, NeboError> {
        let inp: Input =
            serde_json::from_value(input).map_err(|e| NeboError::Execution(e.to_string()))?;

        match inp.action.as_str() {
            "save" => {
                if inp.key.is_empty() {
                    return Err(NeboError::Execution("key is required for save".into()));
                }
                if inp.content.is_empty() {
                    return Err(NeboError::Execution("content is required for save".into()));
                }
                self.notes
                    .lock()
                    .unwrap()
                    .insert(inp.key.clone(), inp.content);
                Ok(format!("Saved note '{}'", inp.key))
            }
            "read" => {
                if inp.key.is_empty() {
                    return Err(NeboError::Execution("key is required for read".into()));
                }
                let notes = self.notes.lock().unwrap();
                match notes.get(&inp.key) {
                    Some(content) => Ok(content.clone()),
                    None => Ok(format!("No note found with key '{}'", inp.key)),
                }
            }
            "list" => {
                let notes = self.notes.lock().unwrap();
                if notes.is_empty() {
                    return Ok("No notes saved.".into());
                }
                let keys: Vec<&str> = notes.keys().map(|k| k.as_str()).collect();
                Ok(keys.join("\n"))
            }
            other => Err(NeboError::Execution(format!("unknown action: {other}"))),
        }
    }
}

#[async_trait]
impl UiHandler for NotepadUi {
    async fn handle_request(&self, req: HttpRequest) -> Result<HttpResponse, NeboError> {
        match req.path.as_str() {
            "api/notes" | "/api/notes" => {
                let notes = self.notes.lock().unwrap();
                let list: Vec<Value> = notes
                    .iter()
                    .map(|(k, v)| {
                        json!({
                            "key": k,
                            "preview": if v.len() > 80 { &v[..80] } else { v.as_str() }
                        })
                    })
                    .collect();
                let body = serde_json::to_vec(&json!({ "notes": list })).unwrap_or_default();
                let mut headers = HashMap::new();
                headers.insert("content-type".to_string(), "application/json".to_string());
                Ok(HttpResponse {
                    status_code: 200,
                    headers,
                    body,
                })
            }
            _ => Ok(HttpResponse {
                status_code: 404,
                headers: HashMap::new(),
                body: b"Not Found".to_vec(),
            }),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let notes: Notes = Arc::new(Mutex::new(HashMap::new()));

    let app = NeboApp::new()?
        .register_tool(NotepadTool {
            notes: Arc::clone(&notes),
        })
        .register_ui(NotepadUi { notes });

    app.run().await?;
    Ok(())
}
