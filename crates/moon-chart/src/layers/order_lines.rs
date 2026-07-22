//! Instance types for order lines and figure geometry in LOGICAL coordinates (time_rel/price);
//! chartdx own-pass shaders map time→x and price→y using chart uniforms. The retained-order
//! workload is tiny (dozens of orders). `crate::build_order_geometry` builds retained-order geometry,
//! while `crate::build_figure_geometry` builds user-defined figure geometry.

/// Instance of a continuous horizontal line (liquidation or figure).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineInstance {
    pub price: f32,
    pub color: [f32; 4],
    pub style: f32, // 0 = solid, 1 = dashed
    pub thickness: f32,
}

/// Instance of an order price zone: a filled band between two prices extending to the right edge.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ZoneInstance {
    pub price0: f32,
    pub price1: f32,
    pub color: [f32; 4],
}

/// Instance of a line segment.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SegInstance {
    pub t0_rel: f32,
    pub p0: f32,
    pub t1_rel: f32,
    pub p1: f32,
    pub thickness: f32,
    /// 0 = solid, 1 = DashDotDot, 2 = Dot (Moonbot trace parity).
    pub pattern: f32,
    /// 1 = the shader takes t1 from the userdata uniform edge (`cv_pad`).
    pub extend: f32,
    pub color: [f32; 4],
}

/// Instance of a marker (cross/knot).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MarkerInstance {
    pub t_rel: f32,
    pub price: f32,
    pub size: f32,
    pub thickness: f32,
    pub shape: f32, // 0 = cross, 1 = knot
    pub color: [f32; 4],
}
