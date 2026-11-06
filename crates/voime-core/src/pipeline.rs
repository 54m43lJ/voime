use std::collections::VecDeque;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use voime_ipc::Mode;

use crate::asr::features::{Fbank, FbankConfig};
use crate::asr::sensevoice::SenseVoice;
use crate::audio::capture::AudioChunk;
use crate::audio::resample::{StreamResampler, TARGET_SAMPLE_RATE};
use crate::config::Config;
use crate::infer::session::SessionFactory;
use crate::model::ModelSet;
use crate::r#final::Final;
use crate::vad::firered::{FireRedVad, VadConfig};
use crate::vad::segmenter::{Segment, SegmentEndReason, Segmenter, SegmenterEvent};

/// Minimum wall-clock spacing between two partial recognitions.
pub const PARTIAL_INTERVAL_MS: u64 = 800;
/// Minimum amount of newly captured audio before the next partial recognition.
pub const PARTIAL_MIN_NEW_AUDIO_MS: u64 = 800;
/// Partial recognition only looks at the tail of a long segment.
pub const PARTIAL_MAX_MS: u64 = 6_000;
/// VAD inference is batched to avoid one ONNX run per 10 ms frame.
pub const VAD_BATCH_MS: u64 = 100;
pub const SEGMENT_QUEUE_CAPACITY: usize = 2;
pub const AUDIO_STREAM_ERROR_LIMIT: u32 = 3;

pub enum PipelineCommand {
    StartSegment,
    FinishSegment,
    CancelPreview,
    SetMode(Mode),
    Shutdown,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PipelineEvent {
    Recording(bool),
    Partial(String),
    SegmentReady(Segment),
    SegmentDropped { reason: String },
    Fatal(String),
}

pub struct Pipeline {
    pub events: crossbeam_channel::Receiver<PipelineEvent>,
    commands: crossbeam_channel::Sender<PipelineCommand>,
    worker: Option<JoinHandle<()>>,
}

pub fn spawn(
    config: &Config,
    factory: SessionFactory,
    audio_rx: crossbeam_channel::Receiver<AudioChunk>,
) -> anyhow::Result<(Pipeline, Final)> {
    let asr_fbank = FbankConfig::sensevoice();
    // Preview and CPU Final own independent sessions. They can run
    // concurrently and never contend on one ORT session lock.
    let preview = SenseVoice::new(&config.models.asr, &factory, asr_fbank)
        .context("创建 Preview SenseVoice 会话失败")?;
    let final_asr = SenseVoice::new(&config.models.asr, &factory, asr_fbank)
        .context("创建 CPU Final SenseVoice 会话失败")?;
    let finalizer = Final::new(final_asr);

    let (commands_tx, commands_rx) = crossbeam_channel::bounded(8);
    let (events_tx, events_rx) = crossbeam_channel::unbounded();
    let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
    let models = config.models.clone();

    let worker = std::thread::Builder::new()
        .name("voime-pipeline".to_string())
        .spawn(
            move || match Worker::new(&models, &factory, preview, asr_fbank, events_tx) {
                Ok(worker) => {
                    let _ = ready_tx.send(Ok(()));
                    worker.run(audio_rx, commands_rx);
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            },
        )
        .context("创建 pipeline 工作线程失败")?;

    match ready_rx.recv() {
        Ok(Ok(())) => Ok((
            Pipeline {
                events: events_rx,
                commands: commands_tx,
                worker: Some(worker),
            },
            finalizer,
        )),
        Ok(Err(error)) => {
            let _ = worker.join();
            Err(error)
        }
        Err(_) => {
            let _ = worker.join();
            bail!("pipeline 工作线程在初始化前退出")
        }
    }
}

impl Pipeline {
    pub fn start_segment(&self) -> anyhow::Result<()> {
        self.commands
            .send(PipelineCommand::StartSegment)
            .map_err(|_| anyhow::anyhow!("发送开始录音命令失败"))
    }

    pub fn finish_segment(&self) -> anyhow::Result<()> {
        self.commands
            .send(PipelineCommand::FinishSegment)
            .map_err(|_| anyhow::anyhow!("发送结束录音命令失败"))
    }

    pub fn cancel_preview(&self) -> anyhow::Result<()> {
        self.commands
            .send(PipelineCommand::CancelPreview)
            .map_err(|_| anyhow::anyhow!("发送取消预览命令失败"))
    }

    pub fn set_mode(&self, mode: Mode) -> anyhow::Result<()> {
        self.commands
            .send(PipelineCommand::SetMode(mode))
            .map_err(|_| anyhow::anyhow!("发送模式命令失败"))
    }

    pub fn shutdown(mut self) -> anyhow::Result<()> {
        let _ = self.commands.send(PipelineCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("pipeline 工作线程异常退出"))?;
        }
        Ok(())
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = self.commands.send(PipelineCommand::Shutdown);
            let _ = worker.join();
        }
    }
}

struct Worker {
    mode: Mode,
    vad: FireRedVad,
    segmenter: Segmenter,
    sensevoice: SenseVoice,
    fbank: FbankConfig,
    resampler: Option<StreamResampler>,
    events: crossbeam_channel::Sender<PipelineEvent>,
    queue: VecDeque<Segment>,
    next_partial_at: Instant,
    next_partial_samples: usize,
    suppress_partials: bool,
    preview_gate: bool,
    collecting: bool,
    listening: bool,
    frame_audio: VecDeque<Vec<f32>>,
    resample_pending: Vec<f32>,
    audio_error_streak: u32,
}

impl Worker {
    fn new(
        models: &ModelSet,
        factory: &SessionFactory,
        sensevoice: SenseVoice,
        asr_fbank: FbankConfig,
        events: crossbeam_channel::Sender<PipelineEvent>,
    ) -> anyhow::Result<Self> {
        let vad = FireRedVad::new(
            &models.vad,
            factory,
            VadConfig {
                fbank: FbankConfig::default(),
                ..VadConfig::default()
            },
        )?;
        let segmenter = Segmenter::new(models.asr.runtime.max_segment_ms);
        Ok(Self {
            mode: Mode::Ptt,
            vad,
            segmenter,
            sensevoice,
            fbank: asr_fbank,
            resampler: None,
            events,
            queue: VecDeque::new(),
            next_partial_at: Instant::now(),
            next_partial_samples: 0,
            suppress_partials: false,
            preview_gate: false,
            collecting: false,
            listening: false,
            frame_audio: VecDeque::new(),
            resample_pending: Vec::new(),
            audio_error_streak: 0,
        })
    }

    fn run(
        mut self,
        audio_rx: crossbeam_channel::Receiver<AudioChunk>,
        commands: crossbeam_channel::Receiver<PipelineCommand>,
    ) {
        loop {
            crossbeam_channel::select_biased! {
                recv(commands) -> message => match message {
                    Ok(command) => {
                        if let Err(error) = self.prepare_command(&command, &audio_rx) {
                            self.send(PipelineEvent::Fatal(format!("{error:#}")));
                            break;
                        }
                        match self.handle_command(command) {
                            Ok(true) => {}
                            Ok(false) => break,
                            Err(error) => {
                                self.send(PipelineEvent::Fatal(format!("{error:#}")));
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                },
                recv(audio_rx) -> message => match message {
                    Ok(chunk) => {
                        if let Err(error) = self.handle_audio(&chunk) {
                            self.send(PipelineEvent::Fatal(format!("{error:#}")));
                            break;
                        }
                    }
                    Err(_) => break,
                },
                default(Duration::from_millis(50)) => {}
            }

            if let Err(error) = self.drain_queue() {
                self.send(PipelineEvent::Fatal(format!("{error:#}")));
                break;
            }
        }
    }

    fn should_process(&self) -> bool {
        self.listening
    }

    fn can_start(&self) -> bool {
        !self.listening && !self.collecting && !self.segmenter.is_active()
    }

    fn handle_audio(&mut self, chunk: &AudioChunk) -> anyhow::Result<()> {
        if chunk.is_stream_error() {
            if !self.should_process() {
                return Ok(());
            }
            self.record_audio_error();
            self.reset_stream_state();
            self.cancel_preview();
            self.send(PipelineEvent::Recording(false));
            self.send(PipelineEvent::SegmentDropped {
                reason: "音频流错误".to_string(),
            });
            if self.audio_error_streak >= AUDIO_STREAM_ERROR_LIMIT {
                bail!("音频流连续失败 {} 次", self.audio_error_streak);
            }
            return Ok(());
        }
        self.audio_error_streak = 0;

        if !self.should_process() {
            self.clear_pending_audio();
            return Ok(());
        }

        if self.resampler.is_none() {
            self.resampler = Some(StreamResampler::new(chunk.sample_rate, chunk.channels));
        }
        let resampler = self.resampler.as_mut().expect("resampler initialized");
        if chunk.sample_rate != resampler.input_rate() || chunk.channels != resampler.channels() {
            self.drop_current_segment("输入设备格式在运行期发生变化");
            self.resampler = Some(StreamResampler::new(chunk.sample_rate, chunk.channels));
            return Ok(());
        }
        let samples = resampler.push(chunk);
        if samples.is_empty() {
            return Ok(());
        }
        self.resample_pending.extend_from_slice(&samples);
        self.process_pending_audio(false)
    }

    fn record_audio_error(&mut self) {
        self.audio_error_streak = self.audio_error_streak.saturating_add(1);
    }

    fn drop_current_segment(&mut self, reason: &str) {
        self.reset_stream_state();
        self.cancel_preview();
        self.send(PipelineEvent::Recording(false));
        self.send(PipelineEvent::SegmentDropped {
            reason: reason.to_string(),
        });
    }

    fn clear_pending_audio(&mut self) {
        self.resample_pending.clear();
        self.frame_audio.clear();
        self.resampler = None;
    }

    fn reset_stream_state(&mut self) {
        self.segmenter.reset();
        self.vad.reset();
        self.frame_audio.clear();
        self.resample_pending.clear();
        self.resampler = None;
        self.reset_partial_schedule();
    }

    fn process_pending_audio(&mut self, flush: bool) -> anyhow::Result<()> {
        if !self.should_process() {
            self.clear_pending_audio();
            return Ok(());
        }
        if flush {
            if let Some(resampler) = self.resampler.as_mut() {
                self.resample_pending.extend_from_slice(&resampler.flush());
            }
        }
        let frame_samples = self.frame_samples();
        let batch_samples = self.vad_batch_samples();
        loop {
            let take = if self.resample_pending.len() >= batch_samples {
                batch_samples
            } else if flush && self.resample_pending.len() >= frame_samples {
                (self.resample_pending.len() / frame_samples) * frame_samples
            } else {
                break;
            };
            let batch: Vec<f32> = self.resample_pending.drain(..take).collect();
            self.process_vad_batch(&batch, frame_samples)?;
        }
        Ok(())
    }

    fn process_vad_batch(&mut self, batch: &[f32], frame_samples: usize) -> anyhow::Result<()> {
        let mut units = std::mem::take(&mut self.frame_audio);
        for unit in batch.chunks_exact(frame_samples) {
            units.push_back(unit.to_vec());
        }
        let frames = match self.vad.push(batch) {
            Ok(frames) => frames,
            Err(_) => {
                self.drop_current_segment("VAD 推理失败");
                return Ok(());
            }
        };
        for frame in frames {
            let audio = units.pop_front().unwrap_or_else(|| {
                let start = batch.len().saturating_sub(frame_samples);
                batch[start..].to_vec()
            });
            self.handle_frame(frame, &audio)?;
            self.drain_queue()?;
        }
        self.frame_audio = units;
        Ok(())
    }

    fn handle_frame(
        &mut self,
        frame: crate::vad::firered::VadFrame,
        audio: &[f32],
    ) -> anyhow::Result<()> {
        if !self.should_process() {
            return Ok(());
        }
        let Some(event) = self.segmenter.push(frame, audio) else {
            if self.preview_gate
                && self.segmenter.is_active()
                && !self.suppress_partials
                && Instant::now() >= self.next_partial_at
                && self.segmenter.active_samples().len() >= self.next_partial_samples
            {
                self.emit_partial()?;
            }
            return Ok(());
        };

        match event {
            SegmenterEvent::SpeechStart => {
                self.preview_gate = true;
                self.next_partial_samples = self.segmenter.active_samples().len()
                    + self.samples_for_ms(PARTIAL_MIN_NEW_AUDIO_MS);
                self.next_partial_at = Instant::now() + Duration::from_millis(PARTIAL_INTERVAL_MS);
                self.send(PipelineEvent::Recording(true));
            }
            SegmenterEvent::SpeechEnd { segment, reason } => {
                self.cancel_preview();
                if !keeps_collecting_after_end(self.mode, reason) {
                    self.enqueue(segment);
                    self.reset_partial_schedule();
                    return Ok(());
                }
                self.emit_segment(segment)?;
                self.reset_after_segment();
                self.send(PipelineEvent::Recording(true));
            }
        }
        Ok(())
    }

    fn prepare_command(
        &mut self,
        command: &PipelineCommand,
        audio_rx: &crossbeam_channel::Receiver<AudioChunk>,
    ) -> anyhow::Result<()> {
        match command {
            PipelineCommand::StartSegment => {
                // StartSegment discards audio captured before the keypress.
                // An ignored Start must not consume the active session's audio.
                if self.can_start() {
                    while audio_rx.try_recv().is_ok() {}
                }
            }
            PipelineCommand::FinishSegment => {
                let previous = self.suppress_partials;
                self.suppress_partials = true;
                let result = (|| -> anyhow::Result<()> {
                    while let Ok(chunk) = audio_rx.try_recv() {
                        self.handle_audio(&chunk)?;
                    }
                    self.process_pending_audio(true)?;
                    self.drain_queue()?;
                    Ok(())
                })();
                self.suppress_partials = previous;
                result?;
            }
            PipelineCommand::CancelPreview
            | PipelineCommand::SetMode(_)
            | PipelineCommand::Shutdown => {}
        }
        Ok(())
    }

    fn handle_command(&mut self, command: PipelineCommand) -> anyhow::Result<bool> {
        match command {
            PipelineCommand::StartSegment => {
                if self.can_start() {
                    self.vad.reset();
                    self.segmenter.reset();
                    self.clear_pending_audio();
                    self.listening = true;
                    self.collecting = self.mode != Mode::Continuous;
                    self.preview_gate = true;
                    self.reset_partial_schedule();
                    self.send(PipelineEvent::Recording(true));
                }
            }
            PipelineCommand::FinishSegment => {
                self.cancel_preview();
                self.listening = false;
                self.collecting = false;
                let segment = self.segmenter.force_end();
                match segment {
                    Some(segment) => self.emit_segment(segment)?,
                    None => {
                        self.send(PipelineEvent::SegmentDropped {
                            reason: "空语音段".to_string(),
                        });
                    }
                }
                self.send(PipelineEvent::Recording(false));
                self.reset_after_finish();
            }
            PipelineCommand::CancelPreview => self.cancel_preview(),
            PipelineCommand::SetMode(mode) => {
                self.mode = mode;
                self.collecting = false;
                self.listening = mode == Mode::Continuous;
                self.clear_pending_audio();
                self.segmenter.reset();
                self.vad.reset();
                self.queue.clear();
                self.reset_partial_schedule();
                self.preview_gate = self.listening;
            }
            PipelineCommand::Shutdown => return Ok(false),
        }
        Ok(true)
    }

    fn cancel_preview(&mut self) {
        self.preview_gate = false;
        self.reset_partial_schedule();
    }

    fn emit_partial(&mut self) -> anyhow::Result<()> {
        if !self.preview_gate || !self.segmenter.is_active() {
            self.reset_partial_schedule();
            return Ok(());
        }
        let started = Instant::now();
        let tail_samples = self.samples_for_ms(PARTIAL_MAX_MS);
        let samples = self.segmenter.active_samples();
        let start = samples.len().saturating_sub(tail_samples);
        let mut fbank = Fbank::new(self.fbank);
        let features = match fbank.compute(&samples[start..]) {
            Ok(features) => features,
            Err(_) => {
                self.finish_partial_schedule(started);
                return Ok(());
            }
        };
        let text = self
            .sensevoice
            .transcribe_features(&features)
            .map(|output| output.text);
        if self.preview_gate {
            if let Ok(text) = text {
                if !text.trim().is_empty() {
                    let text = if start > 0 {
                        format!("…{text}")
                    } else {
                        text
                    };
                    self.send(PipelineEvent::Partial(text));
                }
            }
        }
        self.finish_partial_schedule(started);
        Ok(())
    }

    fn finish_partial_schedule(&mut self, started: Instant) {
        let cost_ms = started.elapsed().as_millis().max(1) as u64;
        let new_audio_ms = PARTIAL_MIN_NEW_AUDIO_MS.max(cost_ms.saturating_mul(2));
        let interval_ms = PARTIAL_INTERVAL_MS.max(cost_ms);
        self.next_partial_samples =
            self.segmenter.active_samples().len() + self.samples_for_ms(new_audio_ms);
        self.next_partial_at = Instant::now() + Duration::from_millis(interval_ms);
    }

    fn reset_partial_schedule(&mut self) {
        self.next_partial_at = Instant::now();
        self.next_partial_samples = 0;
    }

    fn emit_segment(&mut self, segment: Segment) -> anyhow::Result<()> {
        if segment.samples.is_empty() {
            self.send(PipelineEvent::SegmentDropped {
                reason: "空语音段".to_string(),
            });
            return Ok(());
        }
        self.send(PipelineEvent::SegmentReady(segment));
        Ok(())
    }

    fn reset_after_segment(&mut self) {
        self.segmenter.reset();
        self.vad.reset();
        self.frame_audio.clear();
        self.resample_pending.clear();
        self.resampler = None;
        self.reset_partial_schedule();
        self.listening = true;
        self.collecting = true;
        self.preview_gate = true;
    }

    fn reset_after_finish(&mut self) {
        self.segmenter.reset();
        self.vad.reset();
        self.frame_audio.clear();
        self.resample_pending.clear();
        self.resampler = None;
        self.queue.clear();
        self.reset_partial_schedule();
        self.listening = false;
        self.collecting = false;
        self.preview_gate = false;
    }

    fn enqueue(&mut self, segment: Segment) {
        while self.queue.len() >= SEGMENT_QUEUE_CAPACITY {
            self.queue.pop_front();
        }
        self.queue.push_back(segment);
    }

    fn drain_queue(&mut self) -> anyhow::Result<()> {
        while let Some(segment) = self.queue.pop_front() {
            self.emit_segment(segment)?;
        }
        Ok(())
    }

    fn send(&self, event: PipelineEvent) {
        let _ = self.events.send(event);
    }

    fn frame_samples(&self) -> usize {
        ((TARGET_SAMPLE_RATE as u64 * self.fbank.shift_ms as u64) / 1000).max(1) as usize
    }

    fn vad_batch_samples(&self) -> usize {
        let frames = (VAD_BATCH_MS / self.fbank.shift_ms.max(1) as u64).max(1) as usize;
        self.frame_samples().saturating_mul(frames)
    }

    fn samples_for_ms(&self, ms: u64) -> usize {
        ((TARGET_SAMPLE_RATE as u64 * ms) / 1000).max(1) as usize
    }
}

fn keeps_collecting_after_end(mode: Mode, reason: SegmentEndReason) -> bool {
    let _ = reason;
    matches!(mode, Mode::Ptt | Mode::Toggle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_is_bounded() {
        assert_eq!(SEGMENT_QUEUE_CAPACITY, 2);
    }

    #[test]
    fn ptt_and_toggle_keep_collecting_after_end() {
        assert!(keeps_collecting_after_end(
            Mode::Ptt,
            SegmentEndReason::Silence
        ));
        assert!(keeps_collecting_after_end(
            Mode::Toggle,
            SegmentEndReason::MaxDuration
        ));
        assert!(!keeps_collecting_after_end(
            Mode::Continuous,
            SegmentEndReason::Silence
        ));
    }
}
