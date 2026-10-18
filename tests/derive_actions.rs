use inspector_protocol::{FieldValue, Inspector, InspectorActions, Variants};

#[derive(Clone, Copy, Debug, PartialEq, Variants)]
enum Mode {
    First,
    Second,
}

#[derive(Default)]
struct Context {
    calls: usize,
}

fn reset(context: &mut Context) -> Result<String, String> {
    context.calls += 1;
    Ok("reset".into())
}

fn fail(_: &mut Context) -> Result<String, String> {
    Err("operation rejected".into())
}

#[derive(Inspector)]
#[inspector(context = Context)]
#[inspector(button(label = "Reset", run = reset, confirm))]
struct Settings {
    enabled: bool,
    #[inspector_group("Tuning")]
    #[inspector(slider(min = -1.0, max = 1.0, step = 0.05))]
    #[inspector(button(id = "failure", label = "Fail", run = fail, hint = "failure test"))]
    strength: f32,
    #[inspector(dropdown)]
    mode: Mode,
}

#[derive(Inspector)]
#[inspector(button(id = "external", label = "External action"))]
struct MetadataOnly {
    enabled: bool,
}

#[test]
fn ordinary_struct_schema_and_writeback_need_no_engine() {
    let mut settings = Settings {
        enabled: false,
        strength: 0.0,
        mode: Mode::First,
    };
    settings.apply_field("enabled", FieldValue::Bool(true));
    settings.apply_field("strength", FieldValue::Float(0.75));
    settings.apply_field("mode", FieldValue::Enum(1));
    let fields = settings.inspector_fields();
    assert_eq!(fields[0].value, FieldValue::Bool(true));
    assert_eq!(fields[1].value, FieldValue::Float(0.75));
    assert_eq!(fields[1].group, Some("Tuning"));
    assert_eq!(fields[1].step, Some(0.05));
    assert_eq!(fields[2].options, &["First", "Second"]);
    assert_eq!(fields[2].value, FieldValue::Enum(1));
}

#[test]
fn type_and_field_actions_use_explicit_plain_context() {
    let settings = Settings {
        enabled: true,
        strength: 0.0,
        mode: Mode::First,
    };
    assert!(settings.inspector_buttons()[0].confirm);
    assert_eq!(settings.inspector_fields()[1].buttons[0].id, "failure");
    let mut context = Context::default();
    assert_eq!(
        Settings::invoke_action(&mut context, "reset"),
        Ok("reset".into())
    );
    assert_eq!(context.calls, 1);
    assert_eq!(
        Settings::invoke_action(&mut context, "failure"),
        Err("operation rejected".into())
    );
    assert!(Settings::invoke_action(&mut context, "unknown").is_err());
    assert_eq!(context.calls, 1);
}

#[test]
fn metadata_only_buttons_do_not_require_context_or_handler() {
    let settings = MetadataOnly { enabled: true };
    assert_eq!(settings.inspector_buttons()[0].id, "external");
}

#[derive(Clone, Copy)]
struct Vec3([f32; 3]);
impl Vec3 {
    fn to_array(self) -> [f32; 3] {
        self.0
    }
    fn from_array(value: [f32; 3]) -> Self {
        Self(value)
    }
}

#[derive(Clone, Copy)]
struct LinearRgba([f32; 4]);
impl LinearRgba {
    fn to_f32_array(self) -> [f32; 4] {
        self.0
    }
    fn from_f32_array(value: [f32; 4]) -> Self {
        Self(value)
    }
}

#[derive(Inspector)]
struct RichSettings {
    position: Vec3,
    color: LinearRgba,
    curve: inspector_protocol::Curve,
}

#[test]
fn vector_color_and_curve_derive_only_use_consumer_owned_interfaces() {
    let mut settings = RichSettings {
        position: Vec3([0.0; 3]),
        color: LinearRgba([1.0; 4]),
        curve: inspector_protocol::Curve::default(),
    };
    settings.apply_field("position", FieldValue::Vec3([1.0, 2.0, 3.0]));
    settings.apply_field("color", FieldValue::Color([0.1, 0.2, 0.3, 1.0]));
    let curve = inspector_protocol::Curve::linear(&[(0.0, 0.0), (1.0, 1.0)]);
    settings.apply_field("curve", FieldValue::Curve(curve.clone()));
    let fields = settings.inspector_fields();
    assert_eq!(fields[0].value, FieldValue::Vec3([1.0, 2.0, 3.0]));
    assert_eq!(fields[1].value, FieldValue::Color([0.1, 0.2, 0.3, 1.0]));
    assert_eq!(fields[2].value, FieldValue::Curve(curve));
}
