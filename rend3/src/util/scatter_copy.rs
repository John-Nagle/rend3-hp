use std::num::NonZeroU64;

use encase::{private::WriteInto, ShaderSize};
use wgpu::{
    BindGroupLayout, BindingType, Buffer, BufferBindingType, BufferDescriptor, BufferUsages, CommandEncoder,
    ComputePassDescriptor, ComputePipeline, ComputePipelineDescriptor, Device, PipelineCompilationOptions,
    PipelineLayoutDescriptor, ShaderStages,
};

use crate::util::{
    bind_merge::{BindGroupBuilder, BindGroupLayoutBuilder},
    math::div_round_up,
};

pub struct ScatterData<T> {
    pub word_offset: u32,
    pub data: T,
}
impl<T> ScatterData<T> {
    pub fn new(byte_offset: u32, data: T) -> Self {
        Self { word_offset: byte_offset / 4, data }
    }
}

pub struct ScatterCopy {
    pipeline: ComputePipeline,
    bgl: BindGroupLayout,
}
impl ScatterCopy {
    pub fn new(device: &Device) -> Self {
        let sm = device.create_shader_module(wgpu::include_wgsl!("../../shaders/scatter_copy.wgsl"));

        let bgl = BindGroupLayoutBuilder::new()
            .append(
                ShaderStages::COMPUTE,
                BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: Some(NonZeroU64::new(12).unwrap()),
                },
                None,
            )
            .append(
                ShaderStages::COMPUTE,
                BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: Some(NonZeroU64::new(4).unwrap()),
                },
                None,
            )
            .build(device, Some("ScatterCopy bgl"));

        let pll = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("ScatterCopy pll"),
            bind_group_layouts: &[Some(&bgl)],
            /* immediates_ranges: &[], */
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&ComputePipelineDescriptor {
            label: Some("ScatterCopy compute pipeline"),
            layout: Some(&pll),
            module: &sm,
            entry_point: Some("cs_main"),
            compilation_options: PipelineCompilationOptions::default(), // use default WGPU options. New in WGPU 0.20 (JN)
            cache: None, // no pipeline cache. New in WGPU 21
        });

        Self { pipeline, bgl }
    }

    pub fn execute_copy<T, D>(
        &self,
        device: &Device,
        encoder: &mut CommandEncoder,
        destination_buffer: &Buffer,
        data: D,
    ) where
        T: ShaderSize + WriteInto,
        D: IntoIterator<Item = ScatterData<T>>,
        D::IntoIter: ExactSizeIterator,
    {
        //  This is an overly complicated piece of code to copy data into GPU memory.
        //  All this really does is sequentially copy blocks of data into a write-only memory area.
        //  Excessive cleverness with generics and traits makes this unnecessarily difficult and bug-prone. (JN)
        let data_iterator = data.into_iter();

        let size_of_t = T::SHADER_SIZE.get();
        assert_eq!(size_of_t % 4, 0);
        let size_of_t_u32: u32 = size_of_t.try_into().unwrap();

        let count = data_iterator.len() as u64;

        let stride_bytes = size_of_t + 4;
        let _stride_words = (stride_bytes / 4) as usize;

        let buffer_size = count * stride_bytes + 8;
        let source_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("ScatterCopy temporary source buffer"),
            size: buffer_size,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: true,
        });
        //  get_mapped_range_mut now returns a Result.  
        let mut mapped_range = source_buffer
            .slice(..)
            .get_mapped_range_mut()
            .expect("Unable to map range of GPU memory");
        let mut mapped_range = mapped_range.slice(..);     
        //  Output buffer format is an 8-byte header, followed by blocks of a 4-byte header plus data.    
        //  Generate 8-byte header.
        let count_u32: u32 = count.try_into().unwrap();
        let hdr0 = (size_of_t_u32 / 4).to_le_bytes();
        let hdr1 = count_u32.to_le_bytes();
        mapped_range.slice(0..4).copy_from_slice(hdr0.as_slice());
        mapped_range.slice(4..8).copy_from_slice(hdr1.as_slice());
        //  ***RECHECK ALL OFFSETS***
        for (idx, item) in data_iterator.enumerate() {
            // Add four bytes for the fixed header.
            let range_start_bytes = idx * stride_bytes as usize + hdr0.len() + hdr1.len();
            let range_end_bytes = range_start_bytes + stride_bytes as usize;
            //  Add header word for this entry, 4 bytes
            let hdrn = item.word_offset.to_le_bytes();
            mapped_range.slice(range_start_bytes..range_start_bytes+hdrn.len()).copy_from_slice(hdrn.as_slice());
            //  ***CHECK RANGE*** This looks like an off by 1 error. Does stride_bytes contain the hdrn header word?
            let mut writer = encase::internal::Writer::new(&item.data, WriteOnlyBuf(mapped_range.slice(range_start_bytes + hdrn.len() .. range_end_bytes)), 0)
                .expect("Unable to create Writer to GPU");
            item.data.write_into(&mut writer);
        }

        drop(mapped_range);
        source_buffer.unmap();

        let bg = BindGroupBuilder::new().append_buffer(&source_buffer).append_buffer(destination_buffer).build(
            device,
            Some("ScatterCopy temporary bind group"),
            &self.bgl,
        );

        let mut cpass = encoder
            .begin_compute_pass(&ComputePassDescriptor { label: Some("ScatterCopy cpass"), timestamp_writes: None });
        cpass.set_pipeline(&self.pipeline);
        cpass.set_bind_group(0, &bg, &[]);
        cpass.dispatch_workgroups(div_round_up(count_u32, 64), 1, 1);
        drop(cpass);
    }
}

/// Workaround for introduction of WriteOnly type in WGPU.
/// From SkiFire13 on Rust forums.
/// We need a BufferMut which is a WriteOnly.
pub struct WriteOnlyBuf<'a>(pub wgpu::WriteOnly<'a, [u8]>);

impl<'a> encase::internal::BufferMut for WriteOnlyBuf<'a> {
    #[inline]
    fn capacity(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn write<const N: usize>(&mut self, offset: usize, val: &[u8; N]) {
        self.write_slice(offset, val);
    }

    #[inline]
    fn write_slice(&mut self, offset: usize, val: &[u8]) {
        self.0.slice(offset..offset+val.len()).copy_from_slice(val);
    }
}

#[cfg(test)]
mod test {
    use wgpu::util::DeviceExt;

    use crate::util::scatter_copy::{ScatterCopy, ScatterData};

    struct TestContext {
        device: wgpu::Device,
        queue: wgpu::Queue,
    }

    impl TestContext {
        fn new() -> Option<Self> {
            let backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::all());
            let instance =
                wgpu::Instance::new(&wgpu::InstanceDescriptor { backends, ..wgpu::InstanceDescriptor::default() });
            let adapter = pollster::block_on(wgpu::util::initialize_adapter_from_env_or_default(&instance, None))?;
            let (device, queue) = pollster::block_on(adapter.request_device(
                &wgpu::DeviceDescriptor {
                    label: None,
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::default(),
                },
                None,
            ))
            .ok()?;

            Some(Self { device, queue })
        }

        fn buffer<T: bytemuck::Pod>(&self, data: &[T]) -> wgpu::Buffer {
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("target buffer"),
                contents: bytemuck::cast_slice(data),
                usage: wgpu::BufferUsages::all() - wgpu::BufferUsages::MAP_READ - wgpu::BufferUsages::MAP_WRITE,
            })
        }

        fn encoder(&self) -> wgpu::CommandEncoder {
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None })
        }

        fn readback<T: bytemuck::Pod>(
            &self,
            mut encoder: wgpu::CommandEncoder,
            buffer: &wgpu::Buffer,
            bytes: u64,
        ) -> Vec<T> {
            let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("staging"),
                size: bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, bytes);

            self.queue.submit(Some(encoder.finish()));

            staging.slice(..).map_async(wgpu::MapMode::Read, |_| ());
            self.device.poll(wgpu::Maintain::Wait);

            let res = bytemuck::cast_slice(&staging.slice(..).get_mapped_range()).to_vec();

            res
        }
    }

    #[test]
    fn single_word() {
        let Some(ctx) = TestContext::new() else {
            return;
        };

        let scatter = ScatterCopy::new(&ctx.device);

        let buffer = ctx.buffer(&[5.0_f32; 4]);

        let mut encoder = ctx.encoder();

        scatter.execute_copy(&ctx.device, &mut encoder, &buffer, [ScatterData { word_offset: 0, data: 1.0_f32 }]);

        assert_eq!(&ctx.readback::<f32>(encoder, &buffer, 16), &[1.0, 5.0, 5.0, 5.0]);
    }

    #[test]
    fn sparse_words() {
        let Some(ctx) = TestContext::new() else {
            return;
        };

        let scatter = ScatterCopy::new(&ctx.device);

        let buffer = ctx.buffer(&[5.0_f32; 4]);

        let mut encoder = ctx.encoder();

        scatter.execute_copy(
            &ctx.device,
            &mut encoder,
            &buffer,
            [ScatterData { word_offset: 0, data: 1.0_f32 }, ScatterData { word_offset: 2, data: 3.0_f32 }],
        );

        assert_eq!(&ctx.readback::<f32>(encoder, &buffer, 16), &[1.0, 5.0, 3.0, 5.0]);
    }

    #[test]
    fn sparse_multi_words() {
        let Some(ctx) = TestContext::new() else {
            return;
        };

        let scatter = ScatterCopy::new(&ctx.device);

        let buffer = ctx.buffer(&[[9.0_f32; 2]; 4]);

        let mut encoder = ctx.encoder();

        scatter.execute_copy(
            &ctx.device,
            &mut encoder,
            &buffer,
            [
                ScatterData { word_offset: 0, data: [1.0_f32, 2.0_f32] },
                ScatterData { word_offset: 4, data: [5.0_f32, 6.0_f32] },
            ],
        );

        assert_eq!(&ctx.readback::<[f32; 2]>(encoder, &buffer, 32), &[[1.0, 2.0], [9.0, 9.0], [5.0, 6.0], [9.0, 9.0]]);
    }
}
