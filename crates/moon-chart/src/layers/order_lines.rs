//! Типы инстансов линий ордеров — ЛОГИЧЕСКИЕ координаты (time_rel/price); time→x,
//! price→y делает шейдер own-pass (chartdx) по chart-uniform. Объём крошечный
//! (десятки ордеров). Геометрию из ретейн-стора собирает `crate::build_order_geometry`.

/// Инстанс непрерывной горизонтали (ликвидация).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineInstance {
    pub price: f32,
    pub color: [f32; 4],
    pub style: f32, // 0 = сплошная, 1 = пунктир
    pub thickness: f32,
}

/// Инстанс ценовой зоны ордера: filled band между двумя ценами до правого edge.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ZoneInstance {
    pub price0: f32,
    pub price1: f32,
    pub color: [f32; 4],
}

/// Инстанс отрезка линии.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SegInstance {
    pub t0_rel: f32,
    pub p0: f32,
    pub t1_rel: f32,
    pub p1: f32,
    pub thickness: f32,
    /// 0 = solid, 1 = DashDotDot, 2 = Dot (MoonBot trace parity).
    pub pattern: f32,
    /// 1 = t1 берётся из userdata uniform edge (`cv_pad`) в шейдере.
    pub extend: f32,
    pub color: [f32; 4],
}

/// Инстанс маркера (крест/узелок).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MarkerInstance {
    pub t_rel: f32,
    pub price: f32,
    pub size: f32,
    pub thickness: f32,
    pub shape: f32, // 0 = крест, 1 = узелок
    pub color: [f32; 4],
}
