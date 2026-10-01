//! The detector network on the GPU (wgpu compute shaders). Runs the ONNX graph the CPU path
//! (rten) also runs, with the few operators it uses: Conv (+ Relu / Sigmoid), nearest Resize
//! and Concat on channels.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use wgpu::util::DeviceExt;

use super::onnx;

const SHADER: &str = r#"
struct Params {
    cin: u32, cout: u32, h: u32, w: u32,
    oh: u32, ow: u32, k: u32, stride: u32,
    pad: u32, act: u32, scale: u32, n: u32,
}
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> x: array<f32>;
@group(0) @binding(2) var<storage, read> wt: array<f32>;
@group(0) @binding(3) var<storage, read> bias: array<f32>;
@group(0) @binding(4) var<storage, read_write> y: array<f32>;

fn activate(v: f32) -> f32 {
    if (p.act == 1u) { return max(v, 0.0); }
    if (p.act == 2u) { return 1.0 / (1.0 + exp(-v)); }
    return v;
}

// Kernel size (1 or 3), fixed per pipeline so that the loops unroll
override K: u32 = 3u;
// Output channels per invocation, input channels per step through the weights
const OCB: u32 = 16u;
const CIB: u32 = 8u;
var<workgroup> wsh: array<f32, 1152>; // CIB * OCB * 9

// OCB output channels at one place per invocation; the workgroup's weights for CIB input
// channels at a time in workgroup memory
@compute @workgroup_size(16, 8, 1)
fn conv(
    @builtin(global_invocation_id) g: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
    @builtin(workgroup_id) wg: vec3<u32>,
) {
    let inside = g.x < p.ow && g.y < p.oh;
    let oc0 = wg.z * OCB;
    let kk = K * K;
    var acc: array<f32, 16>;
    for (var c0 = 0u; c0 < p.cin; c0 += CIB) {
        for (var i = li; i < CIB * OCB * kk; i += 128u) {
            let q = i % kk;
            let o = (i / kk) % OCB;
            let ci = c0 + i / (kk * OCB);
            let oc = oc0 + o;
            var v = 0.0;
            if (ci < p.cin && oc < p.cout) {
                v = wt[(oc * p.cin + ci) * kk + q];
            }
            wsh[i] = v;
        }
        workgroupBarrier();
        if (inside) {
            for (var cl = 0u; cl < CIB; cl++) {
                let ci = c0 + cl;
                if (ci >= p.cin) { break; }
                var xv: array<f32, 9>;
                for (var ky = 0u; ky < K; ky++) {
                    let iy = i32(g.y * p.stride + ky) - i32(p.pad);
                    for (var kx = 0u; kx < K; kx++) {
                        let ix = i32(g.x * p.stride + kx) - i32(p.pad);
                        var v = 0.0;
                        if (iy >= 0 && iy < i32(p.h) && ix >= 0 && ix < i32(p.w)) {
                            v = x[(ci * p.h + u32(iy)) * p.w + u32(ix)];
                        }
                        xv[ky * K + kx] = v;
                    }
                }
                for (var o = 0u; o < OCB; o++) {
                    let base = (cl * OCB + o) * kk;
                    var s = 0.0;
                    for (var q = 0u; q < kk; q++) {
                        s += xv[q] * wsh[base + q];
                    }
                    acc[o] += s;
                }
            }
        }
        workgroupBarrier();
    }
    if (!inside) { return; }
    let plane = p.oh * p.ow;
    let at = g.y * p.ow + g.x;
    for (var o = 0u; o < OCB; o++) {
        let oc = oc0 + o;
        if (oc < p.cout) {
            y[oc * plane + at] = activate(acc[o] + bias[oc]);
        }
    }
}

// Nearest neighbour upscaling by an integer factor, one channel per z
@compute @workgroup_size(8, 8, 1)
fn resize(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= p.ow || g.y >= p.oh) { return; }
    y[(g.z * p.oh + g.y) * p.ow + g.x] = x[(g.z * p.h + g.y / p.scale) * p.w + g.x / p.scale];
}

@compute @workgroup_size(64, 1, 1)
fn unary(@builtin(global_invocation_id) g: vec3<u32>) {
    let i = g.y * 65535u * 64u + g.x;
    if (i >= p.n) { return; }
    y[i] = activate(x[i]);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    cin: u32,
    cout: u32,
    h: u32,
    w: u32,
    oh: u32,
    ow: u32,
    k: u32,
    stride: u32,
    pad: u32,
    act: u32,
    scale: u32,
    n: u32,
}

const RELU: u32 = 1;
const SIGMOID: u32 = 2;

enum Step {
    Conv {
        x: String,
        y: String,
        weights: wgpu::Buffer,
        bias: wgpu::Buffer,
        cin: usize,
        cout: usize,
        k: usize,
        stride: usize,
        pad: usize,
        act: u32,
    },
    Act {
        x: String,
        y: String,
        act: u32,
    },
    Resize {
        x: String,
        y: String,
        scale: usize,
    },
    Concat {
        xs: Vec<String>,
        y: String,
    },
}

/// A tensor on the GPU: channels, height, width (batch of one)
struct Tensor {
    buffer: wgpu::Buffer,
    shape: [usize; 3],
}

pub struct Net {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Convolutions with 1x1 and 3x3 kernels
    conv1: wgpu::ComputePipeline,
    conv3: wgpu::ComputePipeline,
    resize: wgpu::ComputePipeline,
    unary: wgpu::ComputePipeline,
    steps: Vec<Step>,
    input: String,
    outputs: Vec<String>,
    failed: Arc<AtomicBool>,
}

/// Output of the network: values and shape (channels, height, width) per graph output
pub type Output = Vec<(Vec<f32>, [usize; 3])>;

impl Net {
    /// The network of an ONNX model on the GPU; None without a (hardware) GPU or when the
    /// model uses something not supported here
    pub fn new(model: &[u8]) -> Option<Self> {
        let graph = onnx::parse(model)?;
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .ok()?;
        let info = adapter.get_info();
        // A software adapter is slower than the CPU path
        if info.device_type == wgpu::DeviceType::Cpu {
            return None;
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("fingering detector"),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .ok()?;
        let failed = Arc::new(AtomicBool::new(false));
        let flag = failed.clone();
        device.on_uncaptured_error(Arc::new(move |e| {
            log::warn!("Fingering detector on the GPU: {e}");
            flag.store(true, Ordering::Relaxed);
        }));
        // SAFETY: every array access in SHADER is guarded by its bounds (the tensor shapes
        // passed with each dispatch are those of its buffers) and every loop is bounded. The
        // runtime checks (bounds, loop counters) keep the compiler from unrolling the loops
        // and holding the accumulators in registers, several times slower.
        let module = unsafe {
            device.create_shader_module_trusted(
                wgpu::ShaderModuleDescriptor {
                    label: Some("detector"),
                    source: wgpu::ShaderSource::Wgsl(SHADER.into()),
                },
                wgpu::ShaderRuntimeChecks::unchecked(),
            )
        };
        let pipeline = |entry: &str, constants: &[(&str, f64)]| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants,
                    ..Default::default()
                },
                cache: None,
            })
        };
        let conv1 = pipeline("conv", &[("K", 1.0)]);
        let conv3 = pipeline("conv", &[("K", 3.0)]);
        let (resize, unary) = (pipeline("resize", &[]), pipeline("unary", &[]));
        let steps = plan(&device, &graph)?;
        if failed.load(Ordering::Relaxed) {
            return None;
        }
        log::info!("Fingering detector on the GPU: {}", info.name);
        if std::env::var_os("SCORE_READER_TIMING").is_some() {
            eprintln!("GPU: {} ({:?})", info.name, info.backend);
        }
        Some(Self {
            input: graph.inputs.first()?.clone(),
            outputs: graph.outputs.clone(),
            device,
            queue,
            conv1,
            conv3,
            resize,
            unary,
            steps,
            failed,
        })
    }

    fn storage(&self, floats: usize) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (floats.max(1) * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn dispatch(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        params: Params,
        buffers: &[(u32, &wgpu::Buffer)],
        groups: [u32; 3],
    ) {
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform.as_entire_binding(),
        }];
        entries.extend(buffers.iter().map(|&(binding, b)| wgpu::BindGroupEntry {
            binding,
            resource: b.as_entire_binding(),
        }));
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(groups[0], groups[1], groups[2]);
    }

    /// Run on one page (`h` x `w`, row major); None on a GPU error
    pub fn run(&self, page: &[f32], h: usize, w: usize) -> Option<Output> {
        if self.failed.load(Ordering::Relaxed) {
            return None;
        }
        let mut t: HashMap<&str, Tensor> = HashMap::new();
        let input = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(page),
                usage: wgpu::BufferUsages::STORAGE,
            });
        t.insert(
            &self.input,
            Tensor {
                buffer: input,
                shape: [1, h, w],
            },
        );
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let tiles = |n: usize, size: usize| n.div_ceil(size) as u32;
        for step in &self.steps {
            match step {
                Step::Conv {
                    x,
                    y,
                    weights,
                    bias,
                    cin,
                    cout,
                    k,
                    stride,
                    pad,
                    act,
                } => {
                    let src = t.get(x.as_str())?;
                    let [c, h, w] = src.shape;
                    if c != *cin || h + 2 * pad < *k || w + 2 * pad < *k {
                        return None;
                    }
                    let (oh, ow) = (
                        (h + 2 * pad - k) / stride + 1,
                        (w + 2 * pad - k) / stride + 1,
                    );
                    let out = self.storage(cout * oh * ow);
                    let params = Params {
                        cin: *cin as u32,
                        cout: *cout as u32,
                        h: h as u32,
                        w: w as u32,
                        oh: oh as u32,
                        ow: ow as u32,
                        k: *k as u32,
                        stride: *stride as u32,
                        pad: *pad as u32,
                        act: *act,
                        ..Default::default()
                    };
                    let pipeline = if *k == 1 { &self.conv1 } else { &self.conv3 };
                    self.dispatch(
                        &mut encoder,
                        pipeline,
                        params,
                        &[(1, &src.buffer), (2, weights), (3, bias), (4, &out)],
                        [tiles(ow, 16), tiles(oh, 8), tiles(*cout, 16)],
                    );
                    t.insert(
                        y,
                        Tensor {
                            buffer: out,
                            shape: [*cout, oh, ow],
                        },
                    );
                }
                Step::Act { x, y, act } => {
                    let src = t.get(x.as_str())?;
                    let n = src.shape.iter().product::<usize>();
                    let out = self.storage(n);
                    let groups = n.div_ceil(64);
                    let params = Params {
                        act: *act,
                        n: n as u32,
                        ..Default::default()
                    };
                    self.dispatch(
                        &mut encoder,
                        &self.unary,
                        params,
                        &[(1, &src.buffer), (4, &out)],
                        [groups.min(65535) as u32, tiles(groups, 65535), 1],
                    );
                    let shape = src.shape;
                    t.insert(y, Tensor { buffer: out, shape });
                }
                Step::Resize { x, y, scale } => {
                    let src = t.get(x.as_str())?;
                    let [c, h, w] = src.shape;
                    let (oh, ow) = (h * scale, w * scale);
                    let out = self.storage(c * oh * ow);
                    let params = Params {
                        h: h as u32,
                        w: w as u32,
                        oh: oh as u32,
                        ow: ow as u32,
                        scale: *scale as u32,
                        ..Default::default()
                    };
                    self.dispatch(
                        &mut encoder,
                        &self.resize,
                        params,
                        &[(1, &src.buffer), (4, &out)],
                        [tiles(ow, 8), tiles(oh, 8), c as u32],
                    );
                    t.insert(
                        y,
                        Tensor {
                            buffer: out,
                            shape: [c, oh, ow],
                        },
                    );
                }
                Step::Concat { xs, y } => {
                    let parts: Vec<&Tensor> = xs
                        .iter()
                        .map(|x| t.get(x.as_str()))
                        .collect::<Option<_>>()?;
                    let [_, h, w] = parts.first()?.shape;
                    if parts.iter().any(|p| p.shape[1] != h || p.shape[2] != w) {
                        return None;
                    }
                    let c: usize = parts.iter().map(|p| p.shape[0]).sum();
                    let out = self.storage(c * h * w);
                    let mut at = 0;
                    for p in &parts {
                        let n = (p.shape.iter().product::<usize>() * 4) as u64;
                        encoder.copy_buffer_to_buffer(&p.buffer, 0, &out, at, n);
                        at += n;
                    }
                    t.insert(
                        y,
                        Tensor {
                            buffer: out,
                            shape: [c, h, w],
                        },
                    );
                }
            }
        }

        // Read the outputs back
        let mut reads = Vec::new();
        for name in &self.outputs {
            let src = t.get(name.as_str())?;
            let n = (src.shape.iter().product::<usize>() * 4) as u64;
            let read = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: n.max(4),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            encoder.copy_buffer_to_buffer(&src.buffer, 0, &read, 0, n);
            reads.push((read, src.shape));
        }
        self.queue.submit([encoder.finish()]);
        for (read, _) in &reads {
            read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        }
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .ok()?;
        if self.failed.load(Ordering::Relaxed) {
            return None;
        }
        let mut out = Vec::new();
        for (read, shape) in reads {
            let data = read.slice(..).get_mapped_range().ok()?;
            let values: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
            drop(data);
            read.unmap();
            out.push((values, shape));
        }
        Some(out)
    }
}

/// The steps of the graph, activations folded into the convolutions before them
fn plan(device: &wgpu::Device, g: &onnx::Graph) -> Option<Vec<Step>> {
    let mut uses: HashMap<&str, usize> = HashMap::new();
    for n in &g.nodes {
        for i in &n.inputs {
            *uses.entry(i.as_str()).or_default() += 1;
        }
    }
    for o in &g.outputs {
        *uses.entry(o.as_str()).or_default() += 1;
    }
    let mut constants: HashMap<&str, &onnx::Tensor> =
        g.weights.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let upload = |data: &[f32]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(data),
            usage: wgpu::BufferUsages::STORAGE,
        })
    };
    let mut steps: Vec<Step> = Vec::new();
    for n in &g.nodes {
        let x = n.inputs.first().cloned().unwrap_or_default();
        let y = n.outputs.first()?.clone();
        match n.op.as_str() {
            "Constant" => {
                if let Some(onnx::Attr::Tensor(t)) = n.attrs.get("value") {
                    constants.insert(n.outputs.first()?.as_str(), t);
                }
            }
            "Conv" => {
                let w = constants.get(n.inputs.get(1)?.as_str())?;
                let [cout, cin, kh, kw] = w.dims[..] else {
                    return None;
                };
                let stride = n.ints("strides").unwrap_or(&[1, 1]);
                let pads = n.ints("pads").unwrap_or(&[0, 0, 0, 0]);
                let dil = n.ints("dilations").unwrap_or(&[1, 1]);
                if kh != kw
                    || !(kh == 1 || kh == 3)
                    || n.int("group", 1) != 1
                    || stride.iter().any(|&s| s != stride[0])
                    || pads.iter().any(|&p| p != pads[0])
                    || dil.iter().any(|&d| d != 1)
                {
                    return None;
                }
                let bias = match n.inputs.get(2).filter(|b| !b.is_empty()) {
                    Some(b) => constants.get(b.as_str())?.data.clone(),
                    None => vec![0.0; cout],
                };
                steps.push(Step::Conv {
                    x,
                    y,
                    weights: upload(&w.data),
                    bias: upload(&bias),
                    cin,
                    cout,
                    k: kh,
                    stride: stride[0] as usize,
                    pad: pads[0] as usize,
                    act: 0,
                });
            }
            "Relu" | "Sigmoid" => {
                let act = if n.op == "Relu" { RELU } else { SIGMOID };
                // Into the convolution that is the only user of its result
                if let Some(Step::Conv { y: cy, act: ca, .. }) = steps.last_mut()
                    && *cy == x
                    && *ca == 0
                    && uses.get(x.as_str()) == Some(&1)
                {
                    *cy = y;
                    *ca = act;
                } else {
                    steps.push(Step::Act { x, y, act });
                }
            }
            "Resize" | "Upsample" => {
                let mode = n.str("mode").unwrap_or("nearest");
                let scales = n
                    .inputs
                    .iter()
                    .skip(1)
                    .filter_map(|i| constants.get(i.as_str()))
                    .find(|t| t.data.len() == 4)?;
                let s = scales.data[2];
                if mode != "nearest"
                    || scales.data[..2] != [1.0, 1.0]
                    || scales.data[3] != s
                    || s.fract() != 0.0
                    || s < 1.0
                {
                    return None;
                }
                steps.push(Step::Resize {
                    x,
                    y,
                    scale: s as usize,
                });
            }
            "Concat" => {
                if n.int("axis", 0) != 1 {
                    return None;
                }
                steps.push(Step::Concat {
                    xs: n.inputs.clone(),
                    y,
                });
            }
            op => {
                log::info!("Fingering detector: {op} not supported on the GPU");
                return None;
            }
        }
    }
    Some(steps)
}
