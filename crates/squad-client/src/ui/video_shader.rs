use crate::net::DecodedVideoFrame;
use iced::{
    mouse,
    widget::shader::{wgpu, Primitive, Program, Shader, Storage, Viewport},
    Color, Element, Length, Rectangle,
};
use std::num::NonZeroU64;
use std::sync::Arc;

const VIDEO_WGSL_SHADER: &str = r#"
struct Uniforms {
    video_rect: vec4<f32>,
    bg_color: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> u: Uniforms;

@group(0) @binding(1)
var video_tex: texture_2d<f32>;

@group(0) @binding(2)
var video_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_idx: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32(i32(vertex_idx & 1u) * 4 - 1);
    let y = f32(i32(vertex_idx & 2u) * 2 - 1);
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let safe_size = max(u.video_rect.zw, vec2<f32>(1.0, 1.0));
    let uv = (in.position.xy - u.video_rect.xy) / safe_size;
    let clamped_uv = clamp(uv, vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0));
    let sampled = textureSample(video_tex, video_sampler, clamped_uv);
    let inside = uv.x >= 0.0 && uv.x <= 1.0 && uv.y >= 0.0 && uv.y <= 1.0;
    return select(u.bg_color, vec4<f32>(sampled.rgb, 1.0), inside);
}
"#;

pub fn video_view<'a, Message: 'a>(
    frame: Arc<DecodedVideoFrame>,
    bg_color: Color,
) -> Element<'a, Message> {
    Shader::new(VideoFrameProgram { frame, bg_color })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

#[derive(Debug, Clone)]
struct VideoFrameProgram {
    frame: Arc<DecodedVideoFrame>,
    bg_color: Color,
}

impl<Message> Program<Message> for VideoFrameProgram {
    type State = ();
    type Primitive = VideoFramePrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: mouse::Cursor,
        _bounds: Rectangle,
    ) -> Self::Primitive {
        VideoFramePrimitive {
            frame: Arc::clone(&self.frame),
            bg_color: self.bg_color,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VideoFramePrimitive {
    frame: Arc<DecodedVideoFrame>,
    bg_color: Color,
}

#[derive(Debug)]
struct VideoPipelineState {
    pipeline: wgpu::RenderPipeline,
    pipeline_format: wgpu::TextureFormat,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
    texture: Option<wgpu::Texture>,
    bind_group: Option<wgpu::BindGroup>,
    tex_width: u32,
    tex_height: u32,
    tex_format: wgpu::TextureFormat,
    last_uploaded_seq: u64,
}

impl VideoPipelineState {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("squad_air.video_shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(VIDEO_WGSL_SHADER)),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("squad_air.video_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(32),
                    },
                    count: None,
                },
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("squad_air.video_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("squad_air.video_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("squad_air.video_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("squad_air.video_uniforms"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let tex_format = if format.is_srgb() {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };

        Self {
            pipeline,
            pipeline_format: format,
            bind_group_layout,
            sampler,
            uniform_buf,
            texture: None,
            bind_group: None,
            tex_width: 0,
            tex_height: 0,
            tex_format,
            last_uploaded_seq: 0,
        }
    }
}

fn pack_uniforms(video_rect: [f32; 4], bg_color: [f32; 4]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, &val) in video_rect.iter().chain(bg_color.iter()).enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&val.to_ne_bytes());
    }
    out
}

impl Primitive for VideoFramePrimitive {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        storage: &mut Storage,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let needs_init = match storage.get::<VideoPipelineState>() {
            Some(st) => st.pipeline_format != format,
            None => true,
        };
        if needs_init {
            storage.store(VideoPipelineState::new(device, format));
        }

        let Some(state) = storage.get_mut::<VideoPipelineState>() else {
            return;
        };

        let fw = self.frame.width.max(1);
        let fh = self.frame.height.max(1);
        let expected_bytes = (fw as usize).saturating_mul(fh as usize).saturating_mul(4);
        if self.frame.rgba.len() != expected_bytes {
            return;
        }

        let desired_tex_format = if format.is_srgb() {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };

        if state.texture.is_none()
            || state.tex_width != fw
            || state.tex_height != fh
            || state.tex_format != desired_tex_format
        {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("squad_air.video_frame_texture"),
                size: wgpu::Extent3d {
                    width: fw,
                    height: fh,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: desired_tex_format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("squad_air.video_bind_group"),
                layout: &state.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: state.uniform_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&state.sampler),
                    },
                ],
            });
            state.texture = Some(texture);
            state.bind_group = Some(bind_group);
            state.tex_width = fw;
            state.tex_height = fh;
            state.tex_format = desired_tex_format;
            state.last_uploaded_seq = 0;
        }

        if state.last_uploaded_seq != self.frame.frame_seq {
            if let Some(ref texture) = state.texture {
                queue.write_texture(
                    wgpu::ImageCopyTexture {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &self.frame.rgba,
                    wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(fw * 4),
                        rows_per_image: Some(fh),
                    },
                    wgpu::Extent3d {
                        width: fw,
                        height: fh,
                        depth_or_array_layers: 1,
                    },
                );
                state.last_uploaded_seq = self.frame.frame_seq;
            }
        }

        let scale = viewport.scale_factor() as f32;
        let bx = bounds.x * scale;
        let by = bounds.y * scale;
        let bw = (bounds.width * scale).max(1.0);
        let bh = (bounds.height * scale).max(1.0);

        let src_w = fw as f32;
        let src_h = fh as f32;
        let fit_scale = (bw / src_w).min(bh / src_h);
        let vw = (src_w * fit_scale).max(1.0);
        let vh = (src_h * fit_scale).max(1.0);
        let vx = bx + (bw - vw) * 0.5;
        let vy = by + (bh - vh) * 0.5;

        let bg = if format.is_srgb() {
            self.bg_color.into_linear()
        } else {
            [
                self.bg_color.r,
                self.bg_color.g,
                self.bg_color.b,
                self.bg_color.a,
            ]
        };

        let uniform_bytes = pack_uniforms([vx, vy, vw, vh], bg);
        queue.write_buffer(&state.uniform_buf, 0, &uniform_bytes);
    }

    fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        storage: &Storage,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        if clip_bounds.width == 0 || clip_bounds.height == 0 {
            return;
        }
        let Some(state) = storage.get::<VideoPipelineState>() else {
            return;
        };
        let Some(ref bind_group) = state.bind_group else {
            return;
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("squad_air.video_render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_scissor_rect(
            clip_bounds.x,
            clip_bounds.y,
            clip_bounds.width,
            clip_bounds.height,
        );
        pass.set_viewport(
            clip_bounds.x as f32,
            clip_bounds.y as f32,
            clip_bounds.width as f32,
            clip_bounds.height as f32,
            0.0,
            1.0,
        );
        pass.set_pipeline(&state.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
