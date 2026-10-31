use anyhow::Context;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Large enough to cover a long synchronous ASR pass without dropping
/// microphone blocks at a typical 10–20 ms callback size.
pub const AUDIO_CHANNEL_CAPACITY: usize = 2048;

#[derive(Clone, Debug)]
pub struct AudioChunk {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl AudioChunk {
    /// A zero-sized sentinel delivered by the cpal error callback through the
    /// audio channel. It never represents real microphone data.
    pub fn is_stream_error(&self) -> bool {
        self.sample_rate == 0 && self.channels == 0 && self.samples.is_empty()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("没有可用输入设备")]
    NoInputDevice,
    #[error("未找到输入设备 {name}")]
    DeviceNotFound { name: String },
    #[error("设备枚举失败：{0}")]
    Devices(#[source] cpal::DevicesError),
    #[error("读取默认输入配置失败：{0}")]
    DefaultConfig(#[source] cpal::DefaultStreamConfigError),
    #[error("不支持的样本格式：{0:?}")]
    UnsupportedSampleFormat(cpal::SampleFormat),
    #[error("建立输入流失败：{0}")]
    BuildStream(#[source] cpal::BuildStreamError),
    #[error("启动输入流失败：{0}")]
    PlayStream(#[source] cpal::PlayStreamError),
    #[error("暂停输入流失败：{0}")]
    PauseStream(#[source] cpal::PauseStreamError),
    #[error("音频流错误：{0}")]
    StreamLost(String),
}

pub struct CaptureHandle {
    stream: Option<cpal::Stream>,
    playing: bool,
}

pub fn start(
    device_name: Option<&str>,
    tx: crossbeam_channel::Sender<AudioChunk>,
) -> anyhow::Result<CaptureHandle> {
    let host = cpal::default_host();
    let device = match device_name {
        Some(name) => {
            let mut devices = host.input_devices().map_err(AudioError::Devices)?;
            devices
                .find(|device| device.name().ok().as_deref() == Some(name))
                .ok_or_else(|| AudioError::DeviceNotFound {
                    name: name.to_string(),
                })?
        }
        None => host
            .default_input_device()
            .ok_or(AudioError::NoInputDevice)?,
    };

    let supported = device
        .default_input_config()
        .map_err(AudioError::DefaultConfig)?;
    let sample_format = supported.sample_format();
    let stream_config = supported.config();
    let sample_rate = stream_config.sample_rate.0;
    let channels = stream_config.channels;

    let error_tx = tx.clone();
    let error_callback = move |error: cpal::StreamError| {
        let _ = error_tx.try_send(AudioChunk {
            sample_rate: 0,
            channels: 0,
            samples: Vec::new(),
        });
        let _ = error;
    };

    let stream = match sample_format {
        cpal::SampleFormat::I16 => device.build_input_stream(
            &stream_config,
            move |data: &[i16], _| {
                let samples = data.iter().map(|&sample| sample as f32 / 32768.0).collect();
                send(&tx, sample_rate, channels, samples);
            },
            error_callback,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &stream_config,
            move |data: &[u16], _| {
                let samples = data
                    .iter()
                    .map(|&sample| (sample as f32 - 32768.0) / 32768.0)
                    .collect();
                send(&tx, sample_rate, channels, samples);
            },
            error_callback,
            None,
        ),
        cpal::SampleFormat::F32 => device.build_input_stream(
            &stream_config,
            move |data: &[f32], _| {
                send(&tx, sample_rate, channels, data.to_vec());
            },
            error_callback,
            None,
        ),
        cpal::SampleFormat::I32 => device.build_input_stream(
            &stream_config,
            move |data: &[i32], _| {
                let samples = data
                    .iter()
                    .map(|&sample| sample as f32 / 2147483648.0)
                    .collect();
                send(&tx, sample_rate, channels, samples);
            },
            error_callback,
            None,
        ),
        cpal::SampleFormat::U32 => device.build_input_stream(
            &stream_config,
            move |data: &[u32], _| {
                let samples = data
                    .iter()
                    .map(|&sample| (sample as f64 - 2147483648.0) as f32 / 2147483648.0)
                    .collect();
                send(&tx, sample_rate, channels, samples);
            },
            error_callback,
            None,
        ),
        other => return Err(AudioError::UnsupportedSampleFormat(other).into()),
    }
    .map_err(AudioError::BuildStream)
    .context("构建 cpal 输入流失败")?;

    Ok(CaptureHandle {
        stream: Some(stream),
        playing: false,
    })
}

/// Lists input device names in the order reported by the host.
pub fn list_input_devices() -> anyhow::Result<Vec<String>> {
    let host = cpal::default_host();
    let devices = host.input_devices().map_err(AudioError::Devices)?;
    Ok(devices.filter_map(|device| device.name().ok()).collect())
}

pub fn default_input_device_name() -> anyhow::Result<Option<String>> {
    let host = cpal::default_host();
    Ok(host
        .default_input_device()
        .and_then(|device| device.name().ok()))
}

fn send(
    tx: &crossbeam_channel::Sender<AudioChunk>,
    sample_rate: u32,
    channels: u16,
    samples: Vec<f32>,
) {
    let _ = tx.try_send(AudioChunk {
        sample_rate,
        channels,
        samples,
    });
}

impl CaptureHandle {
    pub fn play(&mut self) -> anyhow::Result<()> {
        if self.playing {
            return Ok(());
        }
        if let Some(stream) = &self.stream {
            stream.play().map_err(AudioError::PlayStream)?;
            self.playing = true;
        }
        Ok(())
    }

    pub fn pause(&mut self) -> anyhow::Result<()> {
        if !self.playing {
            return Ok(());
        }
        if let Some(stream) = &self.stream {
            stream.pause().map_err(AudioError::PauseStream)?;
            self.playing = false;
        }
        Ok(())
    }

    pub fn stop(mut self) {
        let _ = self.pause();
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        let _ = self.pause();
    }
}
