//! Audio output for local playback.
//!
//! librespot's rodio sink panics if no output device is available. Release
//! builds abort on that panic. This sink opens the device when playback starts
//! and reports failures through the UI. Fastpotify can then remain available
//! as a Connect remote until an output appears.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait};
use librespot_playback::audio_backend::{Sink, SinkError, SinkResult};
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;
use librespot_playback::mixer::VolumeGetter;
use librespot_playback::{NUM_CHANNELS, SAMPLE_RATE};
use rodio::Source;

use crate::resample::Resampler;

/// The backend name Settings uses for this sink.
pub const NAME: &str = "rodio";

/// Told about output failures, with a message fit for the interface.
pub type ErrorHook = Arc<dyn Fn(String) + Send + Sync>;

const QUEUE_MS: u32 = 200;

const FADE_HEADROOM_MS: u32 = 60;

pub const FADE_MS_CHOICES: [u32; 4] = [0, 100, 250, 500];

pub type FadeMs = Arc<AtomicU32>;

pub fn shared_fade(ms: u32) -> FadeMs {
    Arc::new(AtomicU32::new(clamp_fade_ms(ms)))
}

pub fn clamp_fade_ms(ms: u32) -> u32 {
    ms.min(*FADE_MS_CHOICES.last().unwrap_or(&0))
}

fn queue_ms(fade_ms: u32) -> u32 {
    QUEUE_MS.max(clamp_fade_ms(fade_ms).saturating_add(FADE_HEADROOM_MS))
}

fn fade_out_frames(fade_ms: u32, sample_rate: u32, queued: u64) -> Option<u64> {
    let frames = ramp_frames(fade_ms, sample_rate).min(queued);
    (frames > 0).then_some(frames)
}

fn ramp_frames(ms: u32, sample_rate: u32) -> u64 {
    u64::from(sample_rate) * u64::from(clamp_fade_ms(ms)) / 1000
}

struct Fade {
    gain: AtomicU32,
    step: AtomicU32,
    target: AtomicU32,
    left: AtomicU64,
    played: AtomicU64,
}

impl Fade {
    fn new() -> Self {
        Self {
            gain: AtomicU32::new(1.0f32.to_bits()),
            step: AtomicU32::new(0.0f32.to_bits()),
            target: AtomicU32::new(1.0f32.to_bits()),
            left: AtomicU64::new(0),
            played: AtomicU64::new(0),
        }
    }

    fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }

    fn set(&self, gain: f32) {
        let gain = gain.clamp(0.0, 1.0);
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
        self.target.store(gain.to_bits(), Ordering::Relaxed);
        self.step.store(0.0f32.to_bits(), Ordering::Relaxed);
        self.left.store(0, Ordering::Relaxed);
    }

    fn ramp(&self, target: f32, frames: u64) {
        let target = target.clamp(0.0, 1.0);
        if frames == 0 {
            self.set(target);
            return;
        }
        let step = (target - self.gain()) / frames as f32;
        self.target.store(target.to_bits(), Ordering::Relaxed);
        self.step.store(step.to_bits(), Ordering::Relaxed);
        self.left.store(frames, Ordering::Relaxed);
    }

    fn advance(&self) -> f32 {
        let gain = self.gain();
        let left = self.left.load(Ordering::Relaxed);
        let next = match left {
            0 => gain,
            1 => {
                self.left.store(0, Ordering::Relaxed);
                f32::from_bits(self.target.load(Ordering::Relaxed))
            }
            _ => {
                self.left.store(left - 1, Ordering::Relaxed);
                let step = f32::from_bits(self.step.load(Ordering::Relaxed));
                (gain + step).clamp(0.0, 1.0)
            }
        };
        self.gain.store(next.to_bits(), Ordering::Relaxed);
        self.played.fetch_add(1, Ordering::Relaxed);
        gain
    }

    fn played(&self) -> u64 {
        self.played.load(Ordering::Relaxed)
    }
}

struct Fading<S> {
    inner: S,
    fade: Arc<Fade>,
    channels: rodio::ChannelCount,
    left: rodio::ChannelCount,
    gain: f32,
}

impl<S: rodio::Source> Fading<S> {
    fn new(inner: S, fade: Arc<Fade>) -> Self {
        let channels = inner.channels().max(1);
        Self {
            inner,
            fade,
            channels,
            left: 0,
            gain: 1.0,
        }
    }
}

impl<S: rodio::Source> Iterator for Fading<S> {
    type Item = rodio::Sample;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        if self.left == 0 {
            self.gain = self.fade.advance();
            self.left = self.channels;
        }
        self.left -= 1;
        Some(sample * self.gain)
    }
}

impl<S: rodio::Source> rodio::Source for Fading<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

/// Maximum time `stop` waits for the queue to drain.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Length of each side of an interrupted-track fade.
const INTERRUPT_FADE: Duration = Duration::from_millis(10);

/// How often playback looks at which output the system calls its default.
const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// Default Windows device buffer length in milliseconds.
///
/// Small platform defaults can click under load (#88). A 100 ms buffer avoids
/// these underruns while keeping controls responsive.
pub const DEFAULT_BUFFER_MS: u32 = 100;

/// Allowed Windows device buffer range. Lower values can click; higher values
/// delay playback controls.
pub const BUFFER_MS_RANGE: std::ops::RangeInclusive<u32> = 20..=500;

/// Coordinates an explicit track replacement with the audio thread.
///
/// librespot deliberately leaves a gapless sink running between tracks. That
/// is right when one track reaches its end, but an explicit skip otherwise
/// leaves the old queued audio in front of the replacement. The old signal is
/// faded on rodio's output thread before its queue is discarded; writes stay
/// gated until librespot reports that the replacement track is loaded.
pub struct AudioControl {
    target: Mutex<AudioTarget>,
    waiting_for_track: AtomicBool,
    reset_output: AtomicBool,
    buffer_ms: u32,
}

#[derive(Default)]
struct AudioTarget {
    sink: Weak<rodio::Sink>,
    envelope: Option<Arc<Envelope>>,
}

impl AudioControl {
    pub fn new(buffer_ms: u32) -> Arc<Self> {
        Arc::new(Self {
            target: Mutex::new(AudioTarget::default()),
            waiting_for_track: AtomicBool::new(false),
            reset_output: AtomicBool::new(false),
            buffer_ms: buffer_ms.clamp(*BUFFER_MS_RANGE.start(), *BUFFER_MS_RANGE.end()),
        })
    }

    /// Fades and discards the current output before a user-requested track
    /// change. Repeated skips share the same handoff.
    pub fn interrupt(&self) {
        if self.waiting_for_track.swap(true, Ordering::SeqCst) {
            return;
        }
        let (sink, envelope) = {
            let target = self.target.lock().unwrap_or_else(PoisonError::into_inner);
            (target.sink.upgrade(), target.envelope.clone())
        };
        if let (Some(sink), Some(envelope)) = (&sink, &envelope) {
            envelope.fade_out();
            let wait =
                Duration::from_millis(u64::from(self.buffer_ms)).saturating_add(INTERRUPT_FADE * 2);
            let deadline = Instant::now() + wait;
            while !envelope.silent() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            // Unlike `clear`, this does not wait for every queued source.
            // The replacement gets a fresh rodio sink on its first write.
            sink.stop();
        }
        self.reset_output.store(true, Ordering::SeqCst);
    }

    /// Opens the write gate once librespot has left the old decoder behind.
    pub fn track_changed(&self) {
        self.waiting_for_track.store(false, Ordering::SeqCst);
    }

    /// Releases the gate if the requested replacement stopped instead.
    pub fn stopped(&self) {
        self.waiting_for_track.store(false, Ordering::SeqCst);
    }

    fn waiting_for_track(&self) -> bool {
        self.waiting_for_track.load(Ordering::SeqCst)
    }

    fn take_reset(&self) -> bool {
        self.reset_output.swap(false, Ordering::SeqCst)
    }

    fn register(&self, sink: &Arc<rodio::Sink>, envelope: Arc<Envelope>) {
        let mut target = self.target.lock().unwrap_or_else(PoisonError::into_inner);
        target.sink = Arc::downgrade(sink);
        target.envelope = Some(envelope);
    }
}

/// A sample-clocked gain shared by every chunk in one rodio queue.
struct Envelope {
    level: AtomicU32,
    target: AtomicU32,
    frames: u32,
}

impl Envelope {
    fn full(sample_rate: u32) -> Arc<Self> {
        let frames = fade_frames(sample_rate);
        Arc::new(Self {
            level: AtomicU32::new(frames),
            target: AtomicU32::new(frames),
            frames,
        })
    }

    fn fade_in(sample_rate: u32) -> Arc<Self> {
        let frames = fade_frames(sample_rate);
        Arc::new(Self {
            level: AtomicU32::new(0),
            target: AtomicU32::new(frames),
            frames,
        })
    }

    fn fade_out(&self) {
        self.target.store(0, Ordering::Relaxed);
    }

    fn silent(&self) -> bool {
        self.level.load(Ordering::Relaxed) == 0
    }

    /// Returns this frame's gain, then moves one frame toward the target.
    fn next_gain(&self) -> f32 {
        let level = self.level.load(Ordering::Relaxed);
        let target = self.target.load(Ordering::Relaxed);
        let next = match level.cmp(&target) {
            std::cmp::Ordering::Less => level + 1,
            std::cmp::Ordering::Greater => level - 1,
            std::cmp::Ordering::Equal => level,
        };
        self.level.store(next, Ordering::Relaxed);
        level as f32 / self.frames as f32
    }
}

fn fade_frames(sample_rate: u32) -> u32 {
    (u64::from(sample_rate) * INTERRUPT_FADE.as_millis() as u64 / 1_000).max(1) as u32
}

/// Applies the shared interruption envelope on rodio's output thread, so it
/// can smooth audio that was already queued when the user changes track.
struct TransitionSource {
    inner: rodio::buffer::SamplesBuffer,
    envelope: Arc<Envelope>,
    channel: usize,
    gain: f32,
}

impl TransitionSource {
    fn new(inner: rodio::buffer::SamplesBuffer, envelope: Arc<Envelope>) -> Self {
        Self {
            inner,
            envelope,
            channel: 0,
            gain: 1.0,
        }
    }
}

impl Iterator for TransitionSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        if self.channel == 0 {
            self.gain = self.envelope.next_gain();
        }
        self.channel = (self.channel + 1) % NUM_CHANNELS as usize;
        Some(sample * self.gain)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl Source for TransitionSource {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

/// The buffer to ask the device for, in frames.
///
/// Clamp to the reported range because CoreAudio rejects unsupported sizes.
/// If a device reports no range, request the configured size; `open_stream`
/// can retry without a fixed size.
fn engine_buffer(
    sample_rate: u32,
    ms: u32,
    supported: cpal::SupportedBufferSize,
) -> cpal::BufferSize {
    let ms = ms.clamp(*BUFFER_MS_RANGE.start(), *BUFFER_MS_RANGE.end());
    let frames = (u64::from(sample_rate) * u64::from(ms) / 1000).max(1) as u32;
    match supported {
        cpal::SupportedBufferSize::Range { min, max } if min <= max && max > 0 => {
            cpal::BufferSize::Fixed(frames.clamp(min.max(1), max))
        }
        _ => cpal::BufferSize::Fixed(frames),
    }
}

pub struct RodioSink {
    /// The output device name from Settings; `None` means the default.
    device: Option<String>,
    output: Option<Output>,
    on_error: ErrorHook,
    /// Player volume, applied at output so changes affect queued audio.
    volume: Box<dyn VolumeGetter + Send>,
    applied_volume: f32,
    /// Watches for changes to the default output.
    watch: Option<DefaultWatch>,
    /// How much sound to ask the device to hold, in milliseconds. Taken
    /// when the stream opens, so a change lands with the next restart.
    buffer_ms: u32,
    fade_ms: FadeMs,
    control: Arc<AudioControl>,
}

struct Output {
    sink: Arc<rodio::Sink>,
    _stream: rodio::OutputStream,
    /// The name of the device the stream was opened on.
    device_name: Option<String>,
    /// Set from the audio thread when the stream dies (device unplugged).
    failed: Arc<AtomicBool>,
    /// The rate the stream runs at, and the converter to it when that is
    /// not Spotify's.
    sample_rate: u32,
    resampler: Option<Resampler>,
    envelope: Arc<Envelope>,
    /// Whether this track has supplied audio since its last stop.
    fed: bool,
    last_write: Option<Instant>,
    fade: Arc<Fade>,
    appended: u64,
}

impl Output {
    fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    fn queued_frames(&self) -> u64 {
        self.appended.saturating_sub(self.fade.played())
    }

    fn queued_ms(&self) -> u32 {
        let ms = self.queued_frames() * 1000 / u64::from(self.sample_rate.max(1));
        ms.min(u64::from(u32::MAX)) as u32
    }
}

impl RodioSink {
    pub fn new(
        device: Option<String>,
        on_error: ErrorHook,
        volume: Box<dyn VolumeGetter + Send>,
        buffer_ms: u32,
        fade_ms: FadeMs,
        control: Arc<AudioControl>,
    ) -> Self {
        Self {
            device,
            output: None,
            on_error,
            volume,
            applied_volume: -1.0,
            watch: None,
            buffer_ms,
            fade_ms,
            control,
        }
    }

    fn fade_ms(&self) -> u32 {
        clamp_fade_ms(self.fade_ms.load(Ordering::Relaxed))
    }

    /// Follows the system default output when no device is selected.
    ///
    /// Windows and macOS need explicit polling. PipeWire and PulseAudio move
    /// streams themselves, while ALSA's answer does not change. Polling runs
    /// off the player thread. `at_once` requests a fresh value at playback
    /// start.
    fn follow_default(&mut self, at_once: bool) {
        if cfg!(target_os = "linux") || self.device.is_some() {
            return;
        }
        let Some(output) = &self.output else {
            return;
        };
        let watch = self.watch.get_or_insert_with(DefaultWatch::start);
        let current = if at_once { watch.ask() } else { watch.name() };
        if current.is_some() && current != output.device_name {
            log::info!(
                "the default audio output is now {}; moving playback to it",
                current.as_deref().unwrap_or("[unknown device]")
            );
            self.output = None;
        }
    }

    fn apply_volume(&mut self) {
        let factor = self.volume.attenuation_factor() as f32;
        if let Some(output) = &self.output
            && factor != self.applied_volume
        {
            output.sink.set_volume(factor);
            self.applied_volume = factor;
        }
    }

    /// Opens the output if it is not open, or if it died since.
    fn ensure_open(&mut self) -> SinkResult<()> {
        if self.output.as_ref().is_some_and(Output::failed) {
            log::warn!("the audio output stopped working; reopening it");
            self.output = None;
        }
        if self.output.is_some() {
            return Ok(());
        }
        match open_output(self.device.as_deref(), self.buffer_ms, &self.control) {
            Ok(output) => {
                self.output = Some(output);
                self.applied_volume = -1.0;
                Ok(())
            }
            Err(error) => {
                let message = error.to_string();
                log::error!("{message}");
                (self.on_error)(message.clone());
                Err(SinkError::ConnectionRefused(message))
            }
        }
    }
}

impl Sink for RodioSink {
    fn start(&mut self) -> SinkResult<()> {
        take_precedence();
        self.follow_default(true);
        self.ensure_open()?;
        self.apply_volume();
        let fade_ms = self.fade_ms();
        if let Some(output) = &mut self.output {
            let frames = ramp_frames(fade_ms, output.sample_rate);
            if frames > 0 {
                output.fade.set(0.0);
                output.fade.ramp(1.0, frames);
            } else {
                output.fade.set(1.0);
            }
            output.sink.play();
        }
        Ok(())
    }

    /// Never fails: librespot exits the process when a sink cannot stop.
    fn stop(&mut self) -> SinkResult<()> {
        let fade_ms = self.fade_ms();
        if let Some(output) = &mut self.output {
            if let Some(frames) =
                fade_out_frames(fade_ms, output.sample_rate, output.queued_frames())
            {
                output.fade.ramp(0.0, frames);
            }
            let deadline = Instant::now() + DRAIN_TIMEOUT;
            while !output.sink.empty() && !output.failed() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            output.sink.pause();
            output.fed = false;
            output.last_write = None;
        }
        Ok(())
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        let samples = packet
            .samples()
            .map_err(|error| SinkError::OnWrite(error.to_string()))?;
        let samples = converter.f64_to_f32(samples);
        if self.control.waiting_for_track() {
            return Ok(());
        }
        let fade_ms = self.fade_ms();
        self.follow_default(false);
        self.ensure_open()?;
        if self.control.take_reset()
            && let Some(output) = &mut self.output
        {
            let sink = Arc::new(rodio::Sink::connect_new(output._stream.mixer()));
            let envelope = Envelope::fade_in(output.sample_rate);
            self.control.register(&sink, Arc::clone(&envelope));
            output.sink = sink;
            output.envelope = envelope;
            output.resampler =
                Resampler::new(SAMPLE_RATE, output.sample_rate, NUM_CHANNELS as usize);
            output.fed = false;
            output.last_write = None;
            output.fade = Arc::new(Fade::new());
            output.appended = 0;
            self.applied_volume = -1.0;
        }
        self.apply_volume();
        let Some(output) = &mut self.output else {
            return Err(SinkError::NotConnected(
                "the audio output is not open".into(),
            ));
        };
        let samples = match &mut output.resampler {
            Some(resampler) => resampler.process(&samples),
            None => samples,
        };
        let now = Instant::now();
        if output.fed && output.sink.empty() && !output.sink.is_paused() {
            let late_ms = output
                .last_write
                .map(|last| now.duration_since(last).as_millis())
                .unwrap_or(0);
            log::warn!("audio queue ran dry; next packet arrived after {late_ms} ms");
        }
        output.appended += (samples.len() / NUM_CHANNELS as usize) as u64;
        let source = rodio::buffer::SamplesBuffer::new(
            NUM_CHANNELS as rodio::ChannelCount,
            output.sample_rate as rodio::SampleRate,
            samples,
        );
        output.sink.append(Fading::new(
            TransitionSource::new(source, Arc::clone(&output.envelope)),
            Arc::clone(&output.fade),
        ));
        output.fed = true;
        output.last_write = Some(now);
        // Let rodio drain a little; without this the whole track would be
        // decoded into memory at once.
        let limit = queue_ms(fade_ms);
        while output.queued_ms() > limit {
            if output.failed() {
                let message = "The audio output stopped working".to_string();
                (self.on_error)(message.clone());
                return Err(SinkError::OnWrite(message));
            }
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}

/// Opens the stream at Spotify's stereo 44.1 kHz, so nothing is converted,
/// else at the device's own rate, which Windows insists on for a shared
/// device, else at whatever rodio can find.
///
/// The first two attempts request the configured buffer. The fallback lets
/// the driver choose its buffer size.
fn open_stream(
    device: &cpal::Device,
    on_error: impl FnMut(cpal::StreamError) + Send + Clone + 'static,
    buffer_ms: u32,
) -> Result<rodio::OutputStream, rodio::StreamError> {
    let supported = device
        .default_output_config()
        .map(|config| *config.buffer_size())
        .unwrap_or(cpal::SupportedBufferSize::Unknown);
    let builder = |sample_rate: u32, buffer: bool| -> Result<_, rodio::StreamError> {
        let builder = rodio::OutputStreamBuilder::from_device(device.clone())?
            .with_channels(NUM_CHANNELS as rodio::ChannelCount)
            .with_sample_rate(sample_rate as rodio::SampleRate)
            .with_error_callback(on_error.clone());
        Ok(if buffer {
            builder.with_buffer_size(engine_buffer(sample_rate, buffer_ms, supported))
        } else {
            builder
        })
    };
    // The fixed engine buffer addresses Windows shared-mode underruns (#88).
    // CoreAudio, ALSA, PulseAudio, and PipeWire keep their proven
    // driver-selected callback periods.
    let fixed_buffer = cfg!(windows);
    if let Ok(stream) = builder(SAMPLE_RATE, fixed_buffer)?.open_stream() {
        return Ok(stream);
    }
    if let Ok(config) = device.default_output_config()
        && let Ok(stream) = builder(config.sample_rate().0, fixed_buffer)?.open_stream()
    {
        return Ok(stream);
    }
    builder(SAMPLE_RATE, false)?.open_stream_or_fallback()
}

/// Raises the Windows decoder thread one step above normal to prevent queued
/// audio from running out under load (#88).
///
/// Linux requires rtkit; CoreAudio owns its real-time callback on macOS.
#[cfg(windows)]
fn take_precedence() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
    };
    // SAFETY: the current thread's pseudo-handle needs no closing, and the
    // call takes nothing else.
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
}

#[cfg(not(windows))]
fn take_precedence() {}

/// Last default-output name, polled on a worker thread because Windows device
/// enumeration can block. The thread ends when the sink is dropped.
struct DefaultWatch(Arc<Mutex<Option<String>>>);

impl DefaultWatch {
    fn start() -> Self {
        let shared = Arc::new(Mutex::new(None));
        let weak = Arc::downgrade(&shared);
        let watching = thread::Builder::new()
            .name("audio-default-watch".into())
            .spawn(move || {
                while let Some(shared) = weak.upgrade() {
                    let name = default_output_name();
                    *shared.lock().unwrap_or_else(PoisonError::into_inner) = name;
                    drop(shared);
                    thread::sleep(DEFAULT_CHECK_INTERVAL);
                }
            });
        if let Err(error) = watching {
            log::warn!("cannot watch the default audio output: {error}");
        }
        Self(shared)
    }

    /// Last polled name, or `None` before the first poll.
    fn name(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Asks right now, on this thread.
    fn ask(&self) -> Option<String> {
        let name = default_output_name();
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = name.clone();
        name
    }
}

fn default_output_name() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .and_then(|device| device.name().ok())
}

#[derive(Debug, thiserror::Error)]
enum OpenError {
    #[error("No audio output device was found. Connect or enable one, then press play again.")]
    NoDevice,
    #[error("Cannot list the audio devices: {0}")]
    Devices(#[from] cpal::DevicesError),
    #[error("Cannot open the audio output: {0}")]
    Stream(#[from] rodio::StreamError),
}

fn open_output(
    preferred: Option<&str>,
    buffer_ms: u32,
    control: &AudioControl,
) -> Result<Output, OpenError> {
    let host = cpal::default_host();
    let device = match preferred.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => {
            let chosen = host
                .output_devices()?
                .find(|device| device.name().is_ok_and(|found| found == name));
            match chosen {
                Some(device) => device,
                None => {
                    log::warn!("audio device {name:?} is not available; using the default");
                    host.default_output_device().ok_or(OpenError::NoDevice)?
                }
            }
        }
        None => host.default_output_device().ok_or(OpenError::NoDevice)?,
    };
    let device_name = device.name().ok();
    log::info!(
        "audio output: {}",
        device_name.as_deref().unwrap_or("[unknown device]")
    );

    let failed = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&failed);
    let on_error = move |error: cpal::StreamError| {
        log::error!("audio stream error: {error}");
        flag.store(true, Ordering::Relaxed);
    };
    let mut stream = open_stream(&device, on_error, buffer_ms)?;
    stream.log_on_drop(false);
    let sample_rate = stream.config().sample_rate();
    let resampler = Resampler::new(SAMPLE_RATE, sample_rate, NUM_CHANNELS as usize);
    if resampler.is_some() {
        log::info!(
            "the output runs at {sample_rate} Hz; the music is converted from {SAMPLE_RATE} Hz"
        );
    }
    let sink = Arc::new(rodio::Sink::connect_new(stream.mixer()));
    let envelope = Envelope::full(sample_rate);
    control.register(&sink, Arc::clone(&envelope));
    Ok(Output {
        sink,
        _stream: stream,
        device_name,
        failed,
        sample_rate,
        resampler,
        envelope,
        fed: false,
        last_write: None,
        fade: Arc::new(Fade::new()),
        appended: 0,
    })
}

#[cfg(test)]
mod tests {

    /// Converts the configured buffer duration to device frames.
    #[test]
    fn the_buffer_follows_the_setting_and_the_rate() {
        let unknown = cpal::SupportedBufferSize::Unknown;
        assert_eq!(
            engine_buffer(44_100, 100, unknown),
            cpal::BufferSize::Fixed(4410),
            "a tenth of a second at 44.1 kHz"
        );
        assert_eq!(
            engine_buffer(48_000, 100, unknown),
            cpal::BufferSize::Fixed(4800),
            "the same tenth of a second at 48 kHz"
        );
        assert_eq!(
            engine_buffer(44_100, 20, unknown),
            cpal::BufferSize::Fixed(882)
        );
    }

    /// Clamps the buffer to the device range required by CoreAudio.
    #[test]
    fn a_device_that_states_its_range_is_kept_inside_it() {
        let range = cpal::SupportedBufferSize::Range { min: 64, max: 2048 };
        assert_eq!(
            engine_buffer(44_100, 100, range),
            cpal::BufferSize::Fixed(2048),
            "held down to what the device can take"
        );
        assert_eq!(
            engine_buffer(44_100, 20, range),
            cpal::BufferSize::Fixed(882),
            "and left alone when it fits"
        );
        let tiny = cpal::SupportedBufferSize::Range {
            min: 4096,
            max: 8192,
        };
        assert_eq!(
            engine_buffer(44_100, 20, tiny),
            cpal::BufferSize::Fixed(4096),
            "and brought up to a device that will not go smaller"
        );
    }

    /// Rule: a settings file with a wild number in it still opens a
    /// stream. The range is the range whoever wrote the file thought of.
    #[test]
    fn a_number_from_outside_the_range_is_brought_back_in() {
        let unknown = cpal::SupportedBufferSize::Unknown;
        assert_eq!(
            engine_buffer(44_100, 0, unknown),
            engine_buffer(44_100, *BUFFER_MS_RANGE.start(), unknown)
        );
        assert_eq!(
            engine_buffer(44_100, 100_000, unknown),
            engine_buffer(44_100, *BUFFER_MS_RANGE.end(), unknown)
        );
    }
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn a_fade_out_reaches_silence_over_its_frames() {
        let fade = Fade::new();
        fade.ramp(0.0, 100);
        for _ in 0..100 {
            assert!(fade.advance() > 0.0, "every frame of the ramp is audible");
        }
        assert_eq!(fade.gain(), 0.0, "and the next one is silent");
    }

    #[test]
    fn a_fade_in_reaches_full_volume_over_its_frames() {
        let fade = Fade::new();
        fade.set(0.0);
        fade.ramp(1.0, 50);
        assert_eq!(fade.advance(), 0.0, "it starts silent");
        for _ in 0..49 {
            fade.advance();
        }
        assert_eq!(fade.gain(), 1.0);
    }

    #[test]
    fn a_finished_ramp_holds_at_its_target() {
        let fade = Fade::new();
        fade.ramp(0.0, 10);
        for _ in 0..100 {
            fade.advance();
        }
        assert_eq!(fade.gain(), 0.0);
    }

    #[test]
    fn a_ramp_with_no_frames_lands_at_once() {
        let fade = Fade::new();
        fade.ramp(0.0, 0);
        assert_eq!(fade.gain(), 0.0);
        assert_eq!(fade.advance(), 0.0);
    }

    #[test]
    fn the_ramp_steps_once_per_frame_not_once_per_sample() {
        let fade = Arc::new(Fade::new());
        fade.ramp(0.0, 4);
        let buffer = rodio::buffer::SamplesBuffer::new(2, 44_100, vec![1.0f32; 8]);
        let faded: Vec<f32> = Fading::new(buffer, Arc::clone(&fade)).collect();
        assert_eq!(faded, vec![1.0, 1.0, 0.75, 0.75, 0.5, 0.5, 0.25, 0.25]);
        assert_eq!(fade.played(), 4, "four frames, not eight samples");
    }

    #[test]
    fn the_queue_holds_more_sound_than_the_longest_fade() {
        assert_eq!(queue_ms(0), QUEUE_MS, "no fade, no extra buffering");
        for ms in FADE_MS_CHOICES {
            assert!(
                queue_ms(ms) > ms,
                "a {ms} ms fade needs more than {ms} ms of queued sound"
            );
        }
        assert!(queue_ms(500) >= queue_ms(250), "longer fades queue more");
    }

    #[test]
    fn a_fade_lasts_its_milliseconds_at_the_output_rate() {
        assert_eq!(ramp_frames(250, 44_100), 11_025);
        assert_eq!(ramp_frames(250, 48_000), 12_000);
        assert_eq!(ramp_frames(0, 44_100), 0, "off means no ramp at all");
    }

    #[test]
    fn a_fade_from_outside_the_choices_is_brought_back_in() {
        assert_eq!(clamp_fade_ms(10_000), 500);
        assert_eq!(clamp_fade_ms(250), 250);
        assert_eq!(clamp_fade_ms(0), 0);
    }

    #[test]
    fn a_pause_with_the_fade_off_does_not_ramp() {
        assert_eq!(fade_out_frames(0, 44_100, 44_100), None);
    }

    #[test]
    fn a_pause_with_nothing_queued_does_not_ramp() {
        assert_eq!(fade_out_frames(250, 44_100, 0), None);
    }

    #[test]
    fn a_pause_fades_over_the_sound_it_has() {
        assert_eq!(fade_out_frames(250, 44_100, 44_100), Some(11_025));
        assert_eq!(fade_out_frames(250, 44_100, 2_000), Some(2_000));
    }

    /// A machine without audio (CI, a PC with nothing plugged in) must get
    /// an error and a message for the interface, never a panic. A machine
    /// with audio opens its default device.
    #[test]
    fn starting_without_a_device_is_an_error_not_a_panic() {
        let reported: Arc<Mutex<Option<String>>> = Arc::default();
        let store = Arc::clone(&reported);
        let mut sink = RodioSink::new(
            Some("no such device".into()),
            Arc::new(move |message| *store.lock().unwrap() = Some(message)),
            Box::new(librespot_playback::mixer::NoOpVolume),
            DEFAULT_BUFFER_MS,
            shared_fade(250),
            AudioControl::new(DEFAULT_BUFFER_MS),
        );
        match sink.start() {
            Ok(()) => assert!(reported.lock().unwrap().is_none()),
            Err(SinkError::ConnectionRefused(message)) => {
                assert_eq!(reported.lock().unwrap().as_deref(), Some(message.as_str()));
            }
            Err(other) => panic!("unexpected error: {other}"),
        }
        assert!(sink.stop().is_ok());
    }

    #[test]
    fn an_interrupted_signal_fades_out_and_a_replacement_fades_in() {
        let rate = 1_000;
        let frames = fade_frames(rate) as usize;
        let samples = vec![1.0; (frames + 2) * NUM_CHANNELS as usize];

        let outgoing = Envelope::full(rate);
        outgoing.fade_out();
        let faded: Vec<_> = TransitionSource::new(
            rodio::buffer::SamplesBuffer::new(NUM_CHANNELS.into(), rate, samples.clone()),
            outgoing,
        )
        .collect();
        assert_eq!(faded[0], 1.0);
        assert_eq!(faded[1], 1.0);
        assert_eq!(faded[frames * NUM_CHANNELS as usize], 0.0);

        let incoming = Envelope::fade_in(rate);
        let faded: Vec<_> = TransitionSource::new(
            rodio::buffer::SamplesBuffer::new(NUM_CHANNELS.into(), rate, samples),
            incoming,
        )
        .collect();
        assert_eq!(faded[0], 0.0);
        assert_eq!(faded[1], 0.0);
        assert_eq!(faded[frames * NUM_CHANNELS as usize], 1.0);
    }
}
