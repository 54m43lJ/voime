use std::collections::VecDeque;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use voime_ipc::Mode;

use crate::asr::features::{Fbank, FbankConfig};
use crate::asr::firered_ctc::FireRedCtc;
use crate::audio::capture::AudioChunk;
use crate::audio::resample::{StreamResampler, TARGET_SAMPLE_RATE};
use crate::config::Config;
use crate::infer::session::SessionFactory;
use crate::model::ModelManifest;
use crate::punc::firered::{FireRedPunc, PuncConfig};
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

/// Whether the current speech session survives a VAD segment end.
fn keeps_collecting_after_end(mode: Mode, reason: SegmentEndReason) -> bool {
    match mode {
        Mode::Ptt => reason == SegmentEndReason::MaxDuration,
        Mode::Toggle => true,
        Mode::Continuous => false,
    }
}

#[derive(Clone, Copy, Debug)]
pub enum PipelineCommand {
    StartSegment,
    FinishSegment,
    SetMode(Mode),
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum PipelineEvent {
    Recording(bool),
    Partial(String),
    Final(String),
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
) -> anyhow::Result<Pipeline> {
    let (commands_tx, commands_rx) = crossbeam_channel::bounded(8);
    let (events_tx, events_rx) = crossbeam_channel::unbounded();
    let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
    let manifest = config.manifest.clone();

    let worker = std::thread::Builder::new()
        .name("voime-pipeline".to_string())
        .spawn(move || match Worker::new(&manifest, &factory, events_tx) {
            Ok(worker) => {
                let _ = ready_tx.send(Ok(()));
                worker.run(audio_rx, commands_rx);
            }
            Err(error) => {
                let _ = ready_tx.send(Err(error));
            }
        })
        .context("创建 pipeline 工作线程失败")?;

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Pipeline {
            events: events_rx,
            commands: commands_tx,
            worker: Some(worker),
        }),
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
            .context("发送开始录音命令失败")
    }

    pub fn finish_segment(&self) -> anyhow::Result<()> {
        self.commands
            .send(PipelineCommand::FinishSegment)
            .context("发送结束录音命令失败")
    }

    pub fn set_mode(&self, mode: Mode) -> anyhow::Result<()> {
        self.commands
            .send(PipelineCommand::SetMode(mode))
            .context("发送模式命令失败")
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
    asr: FireRedCtc,
    punc: FireRedPunc,
    resampler: Option<StreamResampler>,
    events: crossbeam_channel::Sender<PipelineEvent>,
    queue: VecDeque<Segment>,
    next_partial_at: Instant,
    next_partial_samples: usize,
    suppress_partials: bool,
    collecting: bool,
    frame_audio: VecDeque<Vec<f32>>,
    resample_pending: Vec<f32>,
    audio_error_streak: u32,
    fbank: FbankConfig,
}

impl Worker {
    fn new(
        manifest: &ModelManifest,
        factory: &SessionFactory,
        events: crossbeam_channel::Sender<PipelineEvent>,
    ) -> anyhow::Result<Self> {
        let fbank = FbankConfig {
            sample_rate: manifest.sample_rate,
            num_bins: manifest.fbank.num_bins,
            frame_ms: manifest.fbank.frame_ms,
            shift_ms: manifest.fbank.shift_ms,
        };
        let vad_config = VadConfig {
            fbank,
            ..VadConfig::default()
        };
        let vad = FireRedVad::new(&manifest.vad, factory, vad_config)?;
        let asr = FireRedCtc::new_with_factory(&manifest.asr, factory)?;
        let punc = FireRedPunc::new_with_factory(
            &manifest.punc,
            factory,
            PuncConfig {
                max_tokens: manifest.punc.max_tokens,
            },
        )?;
        let segmenter = Segmenter::new(manifest.max_segment_ms);
        Ok(Self {
            mode: Mode::Ptt,
            vad,
            segmenter,
            asr,
            punc,
            resampler: None,
            events,
            queue: VecDeque::new(),
            next_partial_at: Instant::now(),
            next_partial_samples: 0,
            suppress_partials: false,
            collecting: false,
            frame_audio: VecDeque::new(),
            resample_pending: Vec::new(),
            audio_error_streak: 0,
            fbank,
        })
    }

    fn run(
        mut self,
        audio_rx: crossbeam_channel::Receiver<AudioChunk>,
        commands: crossbeam_channel::Receiver<PipelineCommand>,
    ) {
        loop {
            // Commands win over queued audio. In particular, FinishSegment
            // must not wait behind a full audio-channel backlog.
            crossbeam_channel::select_biased! {
                recv(commands) -> message => match message {
                    Ok(command) => {
                        if let Err(error) = self.prepare_command(command, &audio_rx) {
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
        self.mode == Mode::Continuous || self.collecting
    }

    fn handle_audio(&mut self, chunk: &AudioChunk) -> anyhow::Result<()> {
        if chunk.is_stream_error() {
            self.audio_error_streak += 1;
            self.segmenter.reset();
            self.collecting = false;
            self.reset_partial_schedule();
            self.vad.reset();
            self.frame_audio.clear();
            self.resample_pending.clear();
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
            self.resample_pending.clear();
            self.frame_audio.clear();
            return Ok(());
        }

        if self.resampler.is_none() {
            self.resampler = Some(StreamResampler::new(chunk.sample_rate, chunk.channels));
        }
        let resampler = self.resampler.as_mut().expect("resampler initialized");
        if chunk.sample_rate != resampler.input_rate() || chunk.channels != resampler.channels() {
            bail!(
                "输入设备格式在运行期发生变化：{} Hz / {} ch -> {} Hz / {} ch",
                resampler.input_rate(),
                resampler.channels(),
                chunk.sample_rate,
                chunk.channels
            );
        }
        let samples = resampler.push(chunk);
        if samples.is_empty() {
            return Ok(());
        }
        self.resample_pending.extend_from_slice(&samples);
        self.process_pending_audio(false)
    }

    /// Feeds resampled audio to the VAD in batches. With `flush`, a shorter
    /// final batch is accepted too (used before ending a PTT segment).
    fn process_pending_audio(&mut self, flush: bool) -> anyhow::Result<()> {
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
        // Keep this batch's audio locally. A SpeechEnd in the middle of the
        // batch resets `frame_audio`, but the remaining frames still need
        // their own (post-boundary) audio.
        let mut units = std::mem::take(&mut self.frame_audio);
        for unit in batch.chunks_exact(frame_samples) {
            units.push_back(unit.to_vec());
        }
        let frames = match self.vad.push(batch) {
            Ok(frames) => frames,
            Err(_) => {
                self.reset_after_segment();
                self.collecting = false;
                self.send(PipelineEvent::Recording(false));
                self.send(PipelineEvent::SegmentDropped {
                    reason: "VAD 推理失败".to_string(),
                });
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
        let Some(event) = self.segmenter.push(frame, audio, self.mode) else {
            if self.segmenter.is_active()
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
                self.next_partial_samples = self.segmenter.active_samples().len()
                    + self.samples_for_ms(PARTIAL_MIN_NEW_AUDIO_MS);
                self.next_partial_at = Instant::now() + Duration::from_millis(PARTIAL_INTERVAL_MS);
                self.send(PipelineEvent::Recording(true));
            }
            SegmenterEvent::SpeechEnd { segment, reason } => {
                self.send(PipelineEvent::Recording(false));
                match self.mode {
                    Mode::Ptt => {
                        // A long hold is split at max_segment_ms. Keep
                        // collecting so the rest of the hold is not dropped.
                        let keep_collecting = keeps_collecting_after_end(Mode::Ptt, reason);
                        self.collecting = false;
                        if self.finalize(segment).is_err() {
                            self.reset_after_segment();
                            self.send(PipelineEvent::SegmentDropped {
                                reason: "最终识别失败".to_string(),
                            });
                        }
                        if keep_collecting {
                            self.collecting = true;
                            self.reset_partial_schedule();
                            self.send(PipelineEvent::Recording(true));
                        }
                    }
                    Mode::Toggle => {
                        // Toggle is one long session: VAD splits it into
                        // segments, but the session stays active until the
                        // second hotkey press.
                        self.collecting = true;
                        if self.finalize(segment).is_err() {
                            self.reset_after_segment();
                            self.send(PipelineEvent::SegmentDropped {
                                reason: "最终识别失败".to_string(),
                            });
                        }
                        self.reset_partial_schedule();
                        self.send(PipelineEvent::Recording(true));
                    }
                    Mode::Continuous => self.enqueue(segment),
                }
            }
        }
        Ok(())
    }

    fn prepare_command(
        &mut self,
        command: PipelineCommand,
        audio_rx: &crossbeam_channel::Receiver<AudioChunk>,
    ) -> anyhow::Result<()> {
        match command {
            PipelineCommand::StartSegment => {
                // Audio captured before the key press must not leak into the
                // new segment.
                while audio_rx.try_recv().is_ok() {}
            }
            PipelineCommand::FinishSegment => {
                // Include everything captured before the end key even if the
                // command channel became ready first. Partial recognition is
                // suppressed here: this drain is a backlog, not new speech.
                if self.should_process() {
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
            }
            PipelineCommand::SetMode(_) | PipelineCommand::Shutdown => {}
        }
        Ok(())
    }

    fn handle_command(&mut self, command: PipelineCommand) -> anyhow::Result<bool> {
        match command {
            PipelineCommand::StartSegment => {
                if self.mode != Mode::Continuous && !self.collecting && !self.segmenter.is_active()
                {
                    self.vad.reset();
                    self.segmenter.reset();
                    self.frame_audio.clear();
                    self.resample_pending.clear();
                    self.collecting = true;
                    self.reset_partial_schedule();
                    self.send(PipelineEvent::Recording(true));
                }
            }
            PipelineCommand::FinishSegment => {
                if self.collecting || self.segmenter.is_active() {
                    self.collecting = false;
                    let segment = self.segmenter.force_end();
                    self.send(PipelineEvent::Recording(false));
                    match segment {
                        Some(segment) => {
                            if self.finalize(segment).is_err() {
                                self.reset_after_segment();
                                self.send(PipelineEvent::SegmentDropped {
                                    reason: "最终识别失败".to_string(),
                                });
                            }
                        }
                        None => {
                            self.reset_after_segment();
                            self.send(PipelineEvent::SegmentDropped {
                                reason: "空语音段".to_string(),
                            });
                        }
                    }
                }
            }
            PipelineCommand::SetMode(mode) => {
                self.mode = mode;
                if mode != Mode::Continuous && !self.segmenter.is_active() {
                    self.collecting = false;
                    self.resample_pending.clear();
                    self.frame_audio.clear();
                    self.reset_partial_schedule();
                }
            }
            PipelineCommand::Shutdown => return Ok(false),
        }
        Ok(true)
    }

    fn emit_partial(&mut self) -> anyhow::Result<()> {
        if !self.segmenter.is_active() {
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
        if features.is_empty() {
            self.finish_partial_schedule(started);
            return Ok(());
        }
        match self.asr.transcribe(&features) {
            Ok(output) if !output.text.trim().is_empty() => {
                let text = if start > 0 {
                    format!("…{}", output.text)
                } else {
                    output.text
                };
                self.send(PipelineEvent::Partial(text));
            }
            Ok(_) | Err(_) => {}
        }
        self.finish_partial_schedule(started);
        Ok(())
    }

    /// Schedules the next partial relative to the time and audio the previous
    /// one consumed. This prevents the next audio frame from immediately
    /// triggering another inference after a slow partial.
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

    fn finalize(&mut self, segment: Segment) -> anyhow::Result<()> {
        if segment.samples.is_empty() {
            self.reset_after_segment();
            self.send(PipelineEvent::SegmentDropped {
                reason: "空语音段".to_string(),
            });
            return Ok(());
        }
        let mut fbank = Fbank::new(self.fbank);
        let features = fbank.compute(&segment.samples)?;
        if features.is_empty() {
            self.reset_after_segment();
            self.send(PipelineEvent::SegmentDropped {
                reason: "音频过短".to_string(),
            });
            return Ok(());
        }
        let asr = match self.asr.transcribe(&features) {
            Ok(asr) => asr,
            Err(_) => {
                self.reset_after_segment();
                self.send(PipelineEvent::SegmentDropped {
                    reason: "CTC 推理失败".to_string(),
                });
                return Ok(());
            }
        };
        if asr.text.trim().is_empty() {
            self.reset_after_segment();
            self.send(PipelineEvent::SegmentDropped {
                reason: "空识别结果".to_string(),
            });
            return Ok(());
        }
        let text = match self.punc.punctuate(&asr.text) {
            Ok(text) => text,
            Err(_) => {
                self.reset_after_segment();
                self.send(PipelineEvent::SegmentDropped {
                    reason: "标点推理失败".to_string(),
                });
                return Ok(());
            }
        };
        self.reset_after_segment();
        if text.trim().is_empty() {
            self.send(PipelineEvent::SegmentDropped {
                reason: "空识别结果".to_string(),
            });
        } else {
            self.send(PipelineEvent::Final(text));
        }
        Ok(())
    }

    fn reset_after_segment(&mut self) {
        self.segmenter.reset();
        self.vad.reset();
        self.frame_audio.clear();
        self.reset_partial_schedule();
    }

    fn enqueue(&mut self, segment: Segment) {
        while self.queue.len() >= SEGMENT_QUEUE_CAPACITY {
            self.queue.pop_front();
        }
        self.queue.push_back(segment);
    }

    fn drain_queue(&mut self) -> anyhow::Result<()> {
        while let Some(segment) = self.queue.pop_front() {
            if self.finalize(segment).is_err() {
                self.reset_after_segment();
                self.send(PipelineEvent::SegmentDropped {
                    reason: "最终识别失败".to_string(),
                });
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_keeps_collecting_after_silence() {
        assert!(keeps_collecting_after_end(
            Mode::Toggle,
            SegmentEndReason::Silence
        ));
        assert!(keeps_collecting_after_end(
            Mode::Toggle,
            SegmentEndReason::MaxDuration
        ));
    }

    #[test]
    fn ptt_stops_collecting_after_silence_but_survives_max_duration() {
        assert!(!keeps_collecting_after_end(
            Mode::Ptt,
            SegmentEndReason::Silence
        ));
        assert!(keeps_collecting_after_end(
            Mode::Ptt,
            SegmentEndReason::MaxDuration
        ));
    }

    #[test]
    fn continuous_hands_segments_to_the_queue() {
        assert!(!keeps_collecting_after_end(
            Mode::Continuous,
            SegmentEndReason::Silence
        ));
    }
}
