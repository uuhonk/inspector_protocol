//! 普通 Rust 数据用的编译期 inspector schema，不依赖引擎。
//!
//! 这个 crate 只描述*什么*可以被检查 —— 它没有渲染代码，也没有网络代码。
//! UI、ECS 存储访问、网络和持久化由消费者实现。按钮描述不持有执行函数；
//! [`InspectorActions`] 使用调用方显式提供的上下文，不引用 Bevy World。
//!
//! 用法：
//! ```ignore
//! use inspector_protocol::Inspector;
//!
//! #[derive(Inspector)]
//! #[inspector(slider(min = 0.0, max = 2.0))]            // 逐字段属性
//! #[inspector(slider(min = 0.0, max = 1.0, step = 0.05))] // 可选的 step
//! #[inspector_group("Movement")]                       // 打开一个 section
//! struct MyConfig { .. }
//! ```
//!
//! slider 可以带一个可选的 `step` 参数（float 字段默认 0.1，
//! 整数默认 1.0）。整个 [`ControlSpec`] 也可以作为 *preset const* 共享 ——
//! 在任意位置定义它（这个 crate 不提供内置 preset），
//! 然后把它的路径直接传给 `slider(...)`；derive 会把该表达式原样粘贴
//! 进字段元数据，不会去检查它的值：
//! ```ignore
//! use inspector_protocol::{ControlSpec, Inspector};
//!
//! const RANGE_PERCENT: ControlSpec =
//!     ControlSpec::Slider { min: 0.0, max: 100.0 };
//!
//! #[derive(Inspector)]
//! #[inspector(slider(RANGE_PERCENT))]
//! struct MyConfig { level: f32 }
//! ```
//!
//! `#[inspector_group("Title")]` 是一个独立属性，用于*打开*前端的
//! 一个 section：被标注的字段以及它之后的每个字段都归入该 group，
//! 直到下一个 `#[inspector_group(..)]`（第一个之前的字段保持不分组）。
//! 它与控件属性相互独立，所以一个字段可以同时带两者。
//!
//! derive 在编译期收集字段（没有反射）。支持的类型：`bool`、各种整数类型、
//! `f32`/`f64`、`LinearRgba`、glam 向量 `Vec2`/`Vec3`/`Vec4`、[`Curve`]，
//! 以及标注了 `#[inspector(dropdown)]` 的 enum；其他的一切都会被跳过。
//! 写回通过 `apply_field` 完成，它同样由 derive 生成。

pub use inspector_protocol_derive::{Inspector, Variants};

use serde::{Deserialize, Serialize};

/// [`Curve`] 的一个控制点，与 inspector 前端的 `CurvePoint` 对应
/// （Godot 语义）。
///
/// `x` 是时间轴，`y` 是数值；`(lx, ly)` / `(rx, ry)` 是左 / 右切线句柄，
/// 用相对该点的*偏移*表示，单位是 (time, value)。它们会随协议一起传递，
/// 好让 bezier 曲线无损往返，但 [`CurveMode::Linear`] 会忽略它们。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub x: f32,
    pub y: f32,
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
}

impl CurvePoint {
    /// 一个不带切线句柄的点（也就是说，它只在
    /// 线性曲线上才有意义）。
    pub const fn new(x: f32, y: f32) -> Self {
        Self {
            x,
            y,
            lx: 0.0,
            ly: 0.0,
            rx: 0.0,
            ry: 0.0,
        }
    }
}

/// [`Curve`] 在各个控制点之间如何插值。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CurveMode {
    /// 带切线句柄的三次曲线段。
    Bezier,
    /// 控制点之间的直线段 —— 那种有棱角、分面的观感。
    #[default]
    Linear,
}

impl CurveMode {
    /// wire / TOML 里的写法（`"bezier"` / `"linear"`）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bezier => "bezier",
            Self::Linear => "linear",
        }
    }

    /// 解析 wire 写法；除 `"bezier"` 以外的一切都算 linear。
    pub fn from_wire(mode: &str) -> Self {
        match mode {
            "bezier" => Self::Bezier,
            _ => Self::Linear,
        }
    }
}

/// 在 inspector 的 curve widget 里编辑的一条 1 维曲线。
///
/// derive 会把这种类型的字段映射到前端的 `curve` 控件；
/// 游戏侧通过采样（[`Curve::sample`]）或烘焙（[`Curve::bake`]）来消费它 ——
/// 例如烘成 shader 用的 lookup texture。
///
/// widget 会让 `points` 按 `x` 有序且永不为空，但手工编辑过的 TOML 不一定如此，
/// 所以所有采样都做了防御性排序，并且空曲线读出来是一条平的 `0.0` 线。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    pub points: Vec<CurvePoint>,
    pub mode: CurveMode,
}

impl Curve {
    /// 一条穿过给定 `(x, y)` 点的线性曲线。
    pub fn linear(points: &[(f32, f32)]) -> Self {
        Self {
            points: points.iter().map(|&(x, y)| CurvePoint::new(x, y)).collect(),
            mode: CurveMode::Linear,
        }
    }

    /// 曲线在时间 `t` 处的值（超出控制点范围的时间会 clamp
    /// 到第一个 / 最后一个控制点）。
    pub fn sample(&self, t: f32) -> f32 {
        if self.points.is_empty() {
            return 0.0;
        }
        let mut points: Vec<CurvePoint> = self.points.clone();
        points.sort_by(|a, b| a.x.total_cmp(&b.x));
        let first = points[0];
        let last = points[points.len() - 1];
        if t <= first.x || points.len() == 1 {
            return first.y;
        }
        if t >= last.x {
            return last.y;
        }
        // `partition_point` 给出第一个越过 `t` 的点；线段就是
        // `[index - 1, index]`，因为有上面的 clamp，两端都一定存在。
        let index = points.partition_point(|p| p.x <= t);
        let (a, b) = (points[index - 1], points[index]);
        let span = (b.x - a.x).max(f32::EPSILON);
        let u = ((t - a.x) / span).clamp(0.0, 1.0);
        match self.mode {
            CurveMode::Linear => a.y + (b.y - a.y) * u,
            CurveMode::Bezier => {
                // 用 Godot 风格句柄的三次曲线：右句柄偏移线段的第一个控制点，
                // 左句柄偏移第二个。
                let (p0, p1) = ((a.x, a.y), (a.x + a.rx, a.y + a.ry));
                let (p3, p2) = ((b.x, b.y), (b.x + b.lx, b.y + b.ly));
                // widget 保证 x 单调，所以 x(s) 也单调，用二分就能
                // 找到目标时间对应的线段参数。
                let (mut lo, mut hi) = (0.0f32, 1.0f32);
                for _ in 0..24 {
                    let mid = 0.5 * (lo + hi);
                    if bezier_axis(p0.0, p1.0, p2.0, p3.0, mid) < t {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                bezier_axis(p0.1, p1.1, p2.1, p3.1, 0.5 * (lo + hi))
            }
        }
    }

    /// 在时间轴上均匀地烘焙 `samples` 个值 —— 这正是
    /// lookup texture 想要的输入。
    pub fn bake(&self, samples: usize) -> Vec<f32> {
        match samples {
            0 => Vec::new(),
            1 => vec![self.sample(0.5)],
            n => (0..n)
                .map(|i| self.sample(i as f32 / (n - 1) as f32))
                .collect(),
        }
    }
}

/// 参数 `s` 处三次 bezier 的一个轴（`a`/`d` 是端点，
/// `b`/`c` 是控制点）。
fn bezier_axis(a: f32, b: f32, c: f32, d: f32, s: f32) -> f32 {
    let m = 1.0 - s;
    m * m * m * a + 3.0 * m * m * s * b + 3.0 * m * s * s * c + s * s * s * d
}

/// 从被检查的字段读出的 / 写回该字段的一个值。
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Int(i32),
    Float(f32),
    Bool(bool),
    /// 指向 [`FieldInfo::options`] 的下标（enum 下拉框）。
    Enum(usize),
    /// 线性 RGBA（用取色器编辑）。
    Color([f32; 4]),
    /// 2 维向量（glam 的 `Vec2` 等），逐分量编辑。
    Vec2([f32; 2]),
    /// 3 维向量（glam 的 `Vec3`），逐分量编辑。
    Vec3([f32; 3]),
    /// 4 维向量（glam 的 `Vec4`），逐分量编辑。
    Vec4([f32; 4]),
    /// 1 维曲线，用前端的 curve widget 编辑。
    Curve(Curve),
}

impl FieldValue {
    pub fn as_int(&self) -> i32 {
        match self {
            Self::Int(v) => *v,
            _ => 0,
        }
    }

    pub fn as_float(&self) -> f32 {
        match self {
            Self::Float(v) => *v,
            _ => 0.0,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Self::Bool(v) => *v,
            _ => false,
        }
    }

    pub fn as_index(&self) -> usize {
        match self {
            Self::Enum(i) => *i,
            _ => 0,
        }
    }

    pub fn as_color(&self) -> [f32; 4] {
        match self {
            Self::Color(c) => *c,
            _ => [0.0, 0.0, 0.0, 1.0],
        }
    }

    pub fn as_vec2(&self) -> [f32; 2] {
        match self {
            Self::Vec2(v) => *v,
            _ => [0.0; 2],
        }
    }

    pub fn as_vec3(&self) -> [f32; 3] {
        match self {
            Self::Vec3(v) => *v,
            _ => [0.0; 3],
        }
    }

    pub fn as_vec4(&self) -> [f32; 4] {
        match self {
            Self::Vec4(v) => *v,
            _ => [0.0; 4],
        }
    }

    /// 曲线本身（会 clone：曲线自己持有点列表）。非曲线的值
    /// 返回一条空曲线，也就是一条平的 `0.0` 线。
    pub fn as_curve(&self) -> Curve {
        match self {
            Self::Curve(c) => c.clone(),
            _ => Curve::default(),
        }
    }
}

/// 前端应该为一个字段渲染哪种控件。
#[derive(Clone, Debug, PartialEq)]
pub enum ControlSpec {
    Input,
    Slider {
        min: f32,
        max: f32,
    },
    Checkbox,
    Dropdown,
    /// RGBA 颜色（用于 `LinearRgba` 字段）。
    ColorPlane,
    /// 逐分量向量编辑器（用于 glam 的 `Vec2` / `Vec3` / `Vec4`）。
    Vector,
    /// Canvas 曲线编辑器（用于 [`Curve`] 字段）。这两个范围设定 widget 的
    /// 时间（x）轴和数值（y）轴；一道裂口的带符号边缘轮廓想要一个
    /// `-1..1` 的 value 范围，而一条振幅包络想要 `0..1`。
    Curve {
        value_min: f32,
        value_max: f32,
        time_min: f32,
        time_max: f32,
    },
}

/// 单个字段的编译期描述（由 `#[derive(Inspector)]` 产生）。
#[derive(Clone, Debug, PartialEq)]
pub struct FieldInfo {
    pub name: &'static str,
    pub control: ControlSpec,
    pub value: FieldValue,
    pub options: &'static [&'static str],
    /// 对整数输入而言：源类型是否无符号（只允许数字）。
    pub unsigned: bool,
    /// slider 的 step（见 `#[inspector(slider(step = ..))]`），
    /// 非 slider 字段为 `None`。当属性省略 `step` 时，derive 会填入
    /// 按类型区分的默认值（float 0.1，整数 1.0）；
    /// `inspector_protocol_server` 会把它转发进协议 hint。
    pub step: Option<f32>,
    /// 来自 `#[inspector_group("..")]` 的可选 section 标题。前端把
    /// 共享同一个 group 的连续字段渲染在同一个可折叠标题下；
    /// `None` 的字段渲染时不带标题。
    pub group: Option<&'static str>,
    /// 挂在**这个字段**上的按钮（`#[inspector(button(..))]`）：前端把它们渲染在这一
    /// 行之后。空切片 = 这个字段后面没有按钮。
    pub buttons: &'static [InspectorButton],
}

/// enum 的下拉框元数据，由 `#[derive(Variants)]` 生成。
#[allow(dead_code)]
pub trait Variants: Sized {
    fn variants() -> &'static [&'static str];
    fn to_index(&self) -> usize;
    fn from_index(index: usize) -> Self;
}

/// 按钮的纯数据描述；执行上下文由使用方提供。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InspectorAction {
    /// 稳定 id（同一个类型内唯一）。前端回传的就是它。
    pub id: &'static str,
    /// 按钮上的文字。
    pub label: &'static str,
    /// 可选的说明（tooltip / 状态行）。
    pub hint: Option<&'static str>,
    /// 是否要求二次确认。
    pub confirm: bool,
}

/// 字段级和类型级按钮使用相同描述，不持有引擎函数指针。
pub type InspectorButton = InspectorAction;

/// 动作处理与字段 schema 分离，`Context` 可以是普通数据或宿主引擎上下文。
pub trait InspectorActions<Context> {
    fn invoke_action(context: &mut Context, action: &str) -> Result<String, String>;
}

impl InspectorAction {
    pub const fn new(id: &'static str, label: &'static str) -> Self {
        Self {
            id,
            label,
            hint: None,
            confirm: false,
        }
    }

    pub const fn hint(mut self, hint: &'static str) -> Self {
        self.hint = Some(hint);
        self
    }

    pub const fn confirm(mut self) -> Self {
        self.confirm = true;
        self
    }
}

/// 字段可被 inspector 编辑的值；ECS 存储约束由使用方的适配器负责。
pub trait Inspector {
    fn inspector_fields(&self) -> Vec<FieldInfo>;
    fn apply_field(&mut self, field: &str, value: FieldValue);

    /// 挂在**类型**上（而不是某个字段上）的按钮：`#[inspector(button(..))]` 写在
    /// struct 上，前端把它们渲染成面板顶部的一行。默认没有。
    fn inspector_buttons(&self) -> &'static [InspectorButton] {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_curve_interpolates_between_points_and_clamps_outside() {
        let curve = Curve::linear(&[(0.0, 0.0), (0.5, 1.0), (1.0, -1.0)]);

        assert_eq!(curve.sample(-1.0), 0.0);
        assert_eq!(curve.sample(0.0), 0.0);
        assert_eq!(curve.sample(0.25), 0.5);
        assert_eq!(curve.sample(0.5), 1.0);
        assert_eq!(curve.sample(0.75), 0.0);
        assert_eq!(curve.sample(2.0), -1.0);
    }

    /// 一条空曲线（一个从未编辑过曲线的 config）不能 panic，
    /// 且必须读成一条平线。
    #[test]
    fn empty_curve_reads_zero() {
        let curve = Curve::default();
        assert_eq!(curve.sample(0.3), 0.0);
        assert!(curve.bake(0).is_empty());
        assert_eq!(curve.bake(3), vec![0.0, 0.0, 0.0]);
    }

    /// 手写的 TOML 可能把点列成乱序；采样时会排序。
    #[test]
    fn unsorted_points_are_sorted() {
        let curve = Curve::linear(&[(1.0, 2.0), (0.0, 0.0)]);
        assert_eq!(curve.sample(0.5), 1.0);
    }

    /// 烘焙会精确命中两个端点，所以烘焙出的 lookup texture
    /// 不会把曲线平移半个 texel。
    #[test]
    fn bake_samples_the_endpoints() {
        let curve = Curve::linear(&[(0.0, -0.5), (1.0, 0.5)]);
        let baked = curve.bake(5);
        assert_eq!(baked, vec![-0.5, -0.25, 0.0, 0.25, 0.5]);
    }

    /// 水平句柄的 bezier 线段就是直线；把句柄拉离直线
    /// 必须让它弯起来。
    #[test]
    fn bezier_mode_follows_its_handles() {
        let flat = Curve {
            points: vec![
                CurvePoint {
                    x: 0.0,
                    y: 0.0,
                    lx: 0.0,
                    ly: 0.0,
                    rx: 0.5,
                    ry: 0.0,
                },
                CurvePoint {
                    x: 1.0,
                    y: 1.0,
                    lx: -0.5,
                    ly: 0.0,
                    rx: 0.0,
                    ry: 0.0,
                },
            ],
            mode: CurveMode::Bezier,
        };
        assert!(
            (flat.sample(0.5) - 0.5).abs() < 1e-3,
            "{:?}",
            flat.sample(0.5)
        );

        let mut bent = flat.clone();
        bent.points[0].ry = 1.0;
        assert!(bent.sample(0.25) > 0.3, "{:?}", bent.sample(0.25));
    }

    #[test]
    fn curve_mode_wire_spelling_round_trips() {
        assert_eq!(
            CurveMode::from_wire(CurveMode::Bezier.as_str()),
            CurveMode::Bezier
        );
        assert_eq!(
            CurveMode::from_wire(CurveMode::Linear.as_str()),
            CurveMode::Linear
        );
        assert_eq!(CurveMode::from_wire("nonsense"), CurveMode::Linear);
    }
}
