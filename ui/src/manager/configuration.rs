use super::*;
use gpui_kit::prelude::FluentBuilder;
pub struct Field {
    pub key: String,
    pub schema: Value,
    pub input: Entity<InputState>,
}
pub struct Editor {
    pub id: String,
    pub name: String,
    pub fields: Vec<Field>,
    pub values: Value,
    pub schema: Value,
}
impl Editor {
    pub fn new(plugin: &Value, window: &mut Window, cx: &mut App) -> Self {
        let mut fields = Vec::new();
        for (key, schema) in plugin["config_schema"]["properties"]
            .as_object()
            .into_iter()
            .flatten()
        {
            let value = plugin["config"][key].clone();
            let text = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            let input = cx.new(|cx| {
                let mut input = InputState::new(window, cx);
                input.set_value(text, window, cx);
                input
            });
            fields.push(Field {
                key: key.clone(),
                schema: schema.clone(),
                input,
            });
        }
        Self {
            id: plugin["id"].as_str().unwrap_or("").into(),
            name: plugin["name"].as_str().unwrap_or("").into(),
            fields,
            values: plugin["config"].clone(),
            schema: plugin["config_schema"].clone(),
        }
    }
    pub fn value(&self, cx: &App) -> anyhow::Result<Value> {
        let mut value = self.values.clone();
        for field in &self.fields {
            if field.schema["enum"].is_array() || field.schema["type"] == "boolean" {
                continue;
            }
            let text = field.input.read(cx).value();
            value[&field.key] = if field.schema["type"] == "string" {
                json!(text)
            } else {
                serde_json::from_str(&text)?
            };
        }
        Ok(value)
    }
    pub fn render(&self, sender: &async_channel::Sender<Request>) -> AnyElement {
        div().p_5().rounded_xl().bg(crate::appearance::color(0x243044)).flex().flex_col().gap_4()
            .when(self.fields.is_empty(), |d| d.child(label("插件尚未声明配置项。运行插件后，Lua 可以动态提供设置。")))
            .child(div().flex().justify_between().items_center().child(div().text_lg().child(crate::i18n::tr(&format!("{} · 配置",self.name))))
                .child(button("close-plugin-config","关闭","x",json!({"op":"plugin_config_close"}),sender)))
            .children(self.fields.iter().map(|f| {
                let mut field=div().flex().flex_col().gap_2().child(label(f.schema["description"].as_str().unwrap_or(&f.key).to_owned()));
                if let Some(options)=f.schema["enum"].as_array(){
                    field=field.child(div().flex().flex_wrap().gap_2().children(options.iter().enumerate().map(|(index,option)|{
                        let raw=option.as_str().map(str::to_owned).unwrap_or_else(||option.to_string());
                        let title=f.schema["enumNames"][index].as_str().unwrap_or(&raw);
                        button(format!("option-{}-{raw}",f.key),title,if self.values[&f.key]==*option{"circle-check"}else{"circle"},json!({"op":"plugin_config_option","key":f.key,"value":option}),sender)
                    })));
                }else if f.schema["type"]=="boolean"{
                    field=field.child(toggle(format!("field-{}",f.key),self.values[&f.key]==true,json!({"op":"plugin_config_option","key":f.key,"value":self.values[&f.key]!=true}),sender));
                }else{field=field.child(Input::new(&f.input));}
                field
            }))
            .child(div().flex().justify_end().child(button("save-plugin-config","保存配置","check",json!({"op":"plugin_config_save"}),sender))).into_any_element()
    }
}
