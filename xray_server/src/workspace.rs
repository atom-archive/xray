use serde_json;

trait View {
    fn type_name(&self) -> &str;
    fn id(&self) -> usize;
    fn render(&self) -> serde_json::Value;
    fn handle_action(&self, serde_json::Value);
}

pub struct WorkspaceView {
    modal_panel: Option<Box<View>>
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Action {
    ToggleFileFinder,
}

struct FileFinderView {
}

impl View for FileFinderView {
    fn type_name(&self) -> &str { "FileFinderView" }
    fn id(&self) -> usize { 0 }
    fn render(&self) -> serde_json::Value { json!({}) }
    fn handle_action(&self, action: serde_json::Value) {}
}

impl WorkspaceView {
    pub fn new() -> Self {
        WorkspaceView{ modal_panel: None }
    }

    pub fn handle_action(&mut self, view_type: String, view_id: usize, action: serde_json::Value) {
        match serde_json::from_value(action) {
            Ok(Action::ToggleFileFinder) => self.toggle_file_finder(),
            _ => eprintln!("Unrecognized action"),
        }
    }

    fn toggle_file_finder(&mut self) {
        if self.modal_panel.is_some() {
            self.modal_panel = None;
        } else {
            self.modal_panel = Some(Box::new(FileFinderView{}));
        }
    }
}
