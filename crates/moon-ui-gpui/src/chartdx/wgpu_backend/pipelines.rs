//! Создание пайплайнов/шейдеров/сэмплеров и фоновой текстуры
//! (вынос из wgpu_backend.rs, verbatim).

use super::*;

pub(super) fn create_pipelines(device: &wgpu::Device, format: wgpu::TextureFormat) -> Pipelines {
    let background_shader = shader(device, "moon_chart_background_wgsl", BACKGROUND_SHADER);
    let grid_shader = shader(device, "moon_chart_grid_wgsl", GRID_SHADER);
    let cursor_shader = shader(device, "moon_chart_cursor_wgsl", CURSOR_SHADER);
    let crosses_shader = shader(device, "moon_chart_crosses_wgsl", CROSSES_SHADER);
    let price_shader = shader(device, "moon_chart_price_wgsl", PRICE_SHADER);
    let book_shader = shader(device, "moon_chart_book_wgsl", BOOK_SHADER);
    let zone_shader = shader(device, "moon_chart_zone_wgsl", ZONE_SHADER);
    let hline_shader = shader(device, "moon_chart_hline_wgsl", HLINE_SHADER);
    let seg_shader = shader(device, "moon_chart_seg_wgsl", SEG_SHADER);
    let marker_shader = shader(device, "moon_chart_marker_wgsl", MARKER_SHADER);
    let bg_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_bg_layout"),
        entries: &[
            uniform_entry(0, std::mem::size_of::<BackgroundParams>()),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let grid_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_grid_layout"),
        entries: &[uniform_entry(0, std::mem::size_of::<GridParams>())],
    });
    let cursor_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_cursor_layout"),
        entries: &[uniform_entry(0, std::mem::size_of::<CursorParams>())],
    });
    let readout_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_readout_layout"),
        entries: &[storage_entry(0)],
    });
    let view_storage_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_view_storage_layout"),
        entries: &[
            uniform_entry(0, std::mem::size_of::<ChartViewGpu>()),
            storage_entry(1),
        ],
    });
    let book_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_book_layout"),
        entries: &[
            uniform_entry(0, std::mem::size_of::<ChartViewGpu>()),
            uniform_entry(1, std::mem::size_of::<BookStyle>()),
            storage_entry(2),
        ],
    });
    let background = pipeline(
        device,
        format,
        &background_shader,
        &bg_layout,
        "background_vertex",
        "background_fragment",
    );
    let blit = pipeline(
        device,
        format,
        &background_shader,
        &bg_layout,
        "background_vertex",
        "blit_fragment",
    );
    let grid = pipeline(
        device,
        format,
        &grid_shader,
        &grid_layout,
        "grid_vertex",
        "grid_fragment",
    );
    let cursor = pipeline(
        device,
        format,
        &cursor_shader,
        &cursor_layout,
        "cursor_vertex",
        "cursor_fragment",
    );
    let readout_shader = shader(device, "moon_chart_readout_wgsl", READOUT_SHADER);
    let readout_rect = pipeline(
        device,
        format,
        &readout_shader,
        &readout_layout,
        "readout_rect_vertex",
        "readout_rect_fragment",
    );
    let crosses = pipeline(
        device,
        format,
        &crosses_shader,
        &view_storage_layout,
        "crosses_vertex",
        "crosses_fragment",
    );
    let volume = pipeline(
        device,
        format,
        &crosses_shader,
        &view_storage_layout,
        "volume_vertex",
        "volume_fragment",
    );
    let price_last = pipeline(
        device,
        format,
        &price_shader,
        &view_storage_layout,
        "price_line_vertex",
        "price_last_fragment",
    );
    let price_mark = pipeline(
        device,
        format,
        &price_shader,
        &view_storage_layout,
        "price_line_vertex",
        "price_mark_fragment",
    );
    let book_bg = opaque_pipeline(
        device,
        format,
        &book_shader,
        &book_layout,
        "book_bg_vertex",
        "book_bg_fragment",
    );
    let book_bars = pipeline(
        device,
        format,
        &book_shader,
        &book_layout,
        "book_bars_vertex",
        "book_bars_fragment",
    );
    let zone = pipeline(
        device,
        format,
        &zone_shader,
        &view_storage_layout,
        "zone_vertex",
        "zone_fragment",
    );
    let hline = pipeline(
        device,
        format,
        &hline_shader,
        &view_storage_layout,
        "hline_vertex",
        "hline_fragment",
    );
    let seg = pipeline(
        device,
        format,
        &seg_shader,
        &view_storage_layout,
        "seg_vertex",
        "seg_fragment",
    );
    let marker = pipeline(
        device,
        format,
        &marker_shader,
        &view_storage_layout,
        "marker_vertex",
        "marker_fragment",
    );
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("moon_chart_bg_sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let point_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("moon_chart_point_sampler"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    Pipelines {
        bg_layout,
        grid_layout,
        cursor_layout,
        readout_layout,
        view_storage_layout,
        book_layout,
        background,
        blit,
        grid,
        cursor,
        readout_rect,
        crosses,
        volume,
        price_last,
        price_mark,
        book_bg,
        book_bars,
        zone,
        hline,
        seg,
        marker,
        sampler,
        point_sampler,
    }
}

fn shader(device: &wgpu::Device, label: &'static str, source: &'static str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

fn uniform_entry(binding: u32, size: usize) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(size as u64),
        },
        count: None,
    }
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    bind_group_layout: &wgpu::BindGroupLayout,
    vs: &str,
    fs: &str,
) -> wgpu::RenderPipeline {
    pipeline_with_blend(
        device,
        format,
        shader,
        bind_group_layout,
        vs,
        fs,
        Some(alpha_blend_state()),
    )
}

fn opaque_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    bind_group_layout: &wgpu::BindGroupLayout,
    vs: &str,
    fs: &str,
) -> wgpu::RenderPipeline {
    pipeline_with_blend(device, format, shader, bind_group_layout, vs, fs, None)
}

fn pipeline_with_blend(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    bind_group_layout: &wgpu::BindGroupLayout,
    vs: &str,
    fs: &str,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("moon_chart_pipeline_layout"),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("moon_chart_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vs),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fs),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn alpha_blend_state() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

pub(super) fn create_background_texture(device: &wgpu::Device, queue: &wgpu::Queue) -> BackgroundTexture {
    let image = image::load_from_memory(BACKGROUND_PNG)
        .expect("embedded chart background must decode")
        .to_rgba8();
    let size = wgpu::Extent3d {
        width: image.width().max(1),
        height: image.height().max(1),
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("moon_chart_background"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        image.as_raw(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(image.width() * 4),
            rows_per_image: None,
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    BackgroundTexture {
        _texture: texture,
        view,
    }
}
