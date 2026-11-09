//! Permission dialog routing. Only authenticated host commands grant access.
use crate::{app::Application, manager::permissions::Editor};
use anyhow::{Context, Result};
use gpui_kit::App;
use serde_json::{Value, json};
impl Application {
    pub fn permissions_command(&mut self, v: &Value, cx: &mut App) -> Option<Result<Value>> {
        let op = v["op"].as_str()?;
        if ![
            "permissions_open",
            "permissions_close",
            "permissions_toggle",
            "permissions_details",
            "permissions_save",
        ]
        .contains(&op)
        {
            return None;
        }
        Some((|| {
            let mut command = None;
            let mut error = None;
            self.manager(cx,|m,_| {
                if op=="permissions_open" {
                    let plugin=m.backend["plugins"].as_array().and_then(|a|a.iter().find(|p|p["id"]==v["plugin"]));
                    if let Some(plugin)=plugin {
                        m.permission_editor=Some(Editor::new(plugin));m.plugin_config=None;m.page="plugins".into();m.transition();
                    }else{error=Some(anyhow::anyhow!("plugin not found"));}
                    return;
                }
                if op=="permissions_close" {m.permission_editor=None;m.transition();return;}
                let Some(editor)=&mut m.permission_editor else {error=Some(anyhow::anyhow!("permission editor not open"));return;};
                if editor.saving {return;}
                match op {
                    "permissions_toggle" | "permissions_details" => {
                        if let Some(id)=v["id"].as_str() {
                            let collection=if op=="permissions_toggle"{&mut editor.selected}else{&mut editor.expanded};
                            if !collection.remove(id){collection.insert(id.into());}
                        }
                    },
                    "permissions_save" => {
                        let approved=if v["allow_all"]==true {
                            editor.description["items"].as_array().into_iter().flatten().filter_map(|i|i["request"]["id"].as_str().map(str::to_owned)).collect::<Vec<_>>()
                        }else{editor.selected.iter().cloned().collect()};
                        command=Some(json!({"op":"plugin_permissions_save","plugin":editor.id,"revision":editor.description["revision"],"approved":approved}));
                    },
                    _=>{},
                }
            });
            if let Some(error) = error {
                return Err(error);
            }
            if let Some(command) = command {
                anyhow::ensure!(
                    self.backend_status == crate::backend_status::BackendStatus::Running,
                    "{}",
                    crate::i18n::tr("请先启动后台")
                );
                self.backend.command(command).context("save permissions")?;
                self.manager(cx, |m, _| {
                    if let Some(e) = &mut m.permission_editor {
                        e.saving = true;
                    }
                });
            }
            Ok(json!({"queued":true}))
        })())
    }
}
