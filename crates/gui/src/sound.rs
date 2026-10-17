// SPDX-License-Identifier: EPL-2.0

//! Sound effects, as classic's `SoundManager`: a gunshot when a robot fires (louder for
//! more power), a hit when a bullet hits a robot or a bullet, an explosion when a robot
//! dies, and a bump when a robot hits a robot or a wall; panned by where on the field it
//! happens. Classic's own sounds (`sounds/`, EPL).
//!
//! The effects are mixed here and played through one output stream: PulseAudio's own
//! protocol on Linux (the `pulseaudio` crate, pure Rust: PulseAudio and PipeWire serve
//! it, and WSLg too; no ALSA headers needed to build), cpal elsewhere (WASAPI, Core
//! Audio). The device opens (on a thread of its own) when sound is first on; when it
//! can't, the GUI stays silent and says why once.

use robocode_s_protocol::v1;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The turns per second above which battles are silent: faster, the effects only blur.
pub const MAX_TPS: u32 = 100;

/// A sound effect (classic's sound table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    Gunshot,
    BulletHitsRobot,
    BulletHitsBullet,
    RobotDeath,
    RobotCollision,
    WallCollision,
}

impl Effect {
    const ALL: [Effect; 6] = [
        Effect::Gunshot,
        Effect::BulletHitsRobot,
        Effect::BulletHitsBullet,
        Effect::RobotDeath,
        Effect::RobotCollision,
        Effect::WallCollision,
    ];

    /// Classic's sound file for it (`SettingsManager.DEFAULT_FILE_*_SFX`).
    fn wav(self) -> &'static [u8] {
        match self {
            Effect::Gunshot => include_bytes!("../sounds/zap.wav"),
            Effect::BulletHitsRobot | Effect::BulletHitsBullet => {
                include_bytes!("../sounds/shellhit.wav")
            }
            Effect::RobotDeath => include_bytes!("../sounds/explode.wav"),
            Effect::RobotCollision | Effect::WallCollision => {
                include_bytes!("../sounds/13831_adcbicycle_22.wav")
            }
        }
    }

    /// How many of it play at once: classic's clip clones. Another one restarts the
    /// oldest.
    fn voices(self) -> usize {
        match self {
            Effect::Gunshot => 5,
            Effect::BulletHitsRobot | Effect::RobotDeath => 3,
            _ => 2,
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&e| e == self).unwrap_or(0)
    }
}

/// An effect to play: how loud (0 to 1) and where (-1 left to 1 right).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cue {
    pub effect: Effect,
    pub volume: f32,
    pub pan: f32,
}

/// The effects of a turn, as classic's `SoundManager.onTurnEnded`: each bullet in its
/// first frame of a state (fired, hit a robot or a bullet, a robot's death explosion; a
/// wall hit is silent), and each robot that hit a wall, and one robot collision.
pub fn cues(snapshot: &v1::TurnSnapshot, width: f64) -> Vec<Cue> {
    let pan = |x: Option<f64>| {
        let half = width / 2.0;
        if half > 0.0 {
            ((x.unwrap_or(half) - half) / half).clamp(-1.0, 1.0) as f32
        } else {
            0.0
        }
    };
    let mut out = Vec::new();
    for b in snapshot.bullets.iter().filter(|b| b.frame == 0) {
        let (effect, volume) = match v1::BulletState::try_from(b.state) {
            // Rules.MAX_BULLET_POWER: 3.
            Ok(v1::BulletState::Fired) => (
                Effect::Gunshot,
                (b.power.unwrap_or(3.0) / 3.0).clamp(0.0, 1.0) as f32,
            ),
            Ok(v1::BulletState::HitVictim) => (Effect::BulletHitsRobot, 1.0),
            Ok(v1::BulletState::HitBullet) => (Effect::BulletHitsBullet, 1.0),
            Ok(v1::BulletState::Exploded) => (Effect::RobotDeath, 1.0),
            _ => continue,
        };
        out.push(Cue {
            effect,
            volume,
            pan: pan(b.x),
        });
    }
    let mut collided = false;
    for r in &snapshot.robots {
        let effect = match v1::RobotState::try_from(r.state) {
            // Both robots of a collision hit each other: one sound.
            Ok(v1::RobotState::HitRobot) if !collided => {
                collided = true;
                Effect::RobotCollision
            }
            Ok(v1::RobotState::HitWall) => Effect::WallCollision,
            _ => continue,
        };
        out.push(Cue {
            effect,
            volume: 1.0,
            pan: pan(r.x),
        });
    }
    out
}

/// Classic's volume: a gain of a quarter of the line's minimum (-80 dB) times
/// (1 - volume), so a power 0.1 gunshot plays at -19 dB. As an amplitude.
fn amplitude(volume: f32) -> f32 {
    10f32.powf(-(1.0 - volume.clamp(0.0, 1.0)))
}

/// A sound: mono samples (-1 to 1) at a rate.
struct Samples {
    data: Vec<f32>,
    rate: u32,
}

/// The samples of a PCM WAV file (8 or 16 bits, any channels, mixed to mono).
fn decode_wav(bytes: &[u8]) -> Result<Samples, String> {
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let (mut format, mut data) = (None, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32_at(at + 4) as usize;
        let body = at + 8..(at + 8 + size).min(bytes.len());
        match &bytes[at..at + 4] {
            b"fmt " if size >= 16 => {
                let b = body.start;
                format = Some((u16_at(b), u16_at(b + 2), u32_at(b + 4), u16_at(b + 14)));
            }
            b"data" => data = Some(&bytes[body]),
            _ => {}
        }
        at += 8 + size + size % 2;
    }
    let (Some((tag, channels, rate, bits)), Some(data)) = (format, data) else {
        return Err("no format or data".into());
    };
    if tag != 1 || channels == 0 || rate == 0 {
        return Err(format!("unsupported WAV format {tag}"));
    }
    let channels = usize::from(channels);
    let samples: Vec<f32> = match bits {
        8 => data
            .iter()
            .map(|&s| (f32::from(s) - 128.0) / 128.0)
            .collect(),
        16 => data
            .chunks_exact(2)
            .map(|s| f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.0)
            .collect(),
        _ => return Err(format!("unsupported {bits}-bit WAV")),
    };
    let data = samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();
    Ok(Samples { data, rate })
}

/// A sound playing: where it is (in its own samples) and its gain per side.
struct Voice {
    effect: Effect,
    at: f64,
    gain: [f32; 2],
    /// When it started (a count), to restart the oldest.
    started: u64,
}

/// Mixes the playing effects into stereo frames.
pub struct Mixer {
    sounds: Vec<Arc<Samples>>,
    voices: Vec<Voice>,
    started: u64,
    /// The output's frames per second.
    rate: u32,
}

impl Mixer {
    fn new(rate: u32) -> Self {
        let sounds = Effect::ALL
            .iter()
            .map(|e| {
                Arc::new(decode_wav(e.wav()).unwrap_or(Samples {
                    data: Vec::new(),
                    rate: 1,
                }))
            })
            .collect();
        Self {
            sounds,
            voices: Vec::new(),
            started: 0,
            rate: rate.max(1),
        }
    }

    /// Starts a cue at `master` volume (0 to 1): pan as classic's balance, which keeps
    /// the near side at full and turns the far side down.
    fn play(&mut self, cue: Cue, master: f32) {
        let same: Vec<usize> = (0..self.voices.len())
            .filter(|&i| self.voices[i].effect == cue.effect)
            .collect();
        if same.len() >= cue.effect.voices()
            && let Some(&oldest) = same.iter().min_by_key(|&&i| self.voices[i].started)
        {
            self.voices.swap_remove(oldest);
        }
        let gain = amplitude(cue.volume) * master.clamp(0.0, 1.0);
        let pan = cue.pan.clamp(-1.0, 1.0);
        self.started += 1;
        self.voices.push(Voice {
            effect: cue.effect,
            at: 0.0,
            gain: [gain * (1.0 - pan).min(1.0), gain * (1.0 + pan).min(1.0)],
            started: self.started,
        });
    }

    /// Fills `out` with interleaved frames of `channels` (the first two get left and
    /// right; mono gets both).
    fn fill(&mut self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        let channels = channels.max(1);
        let rate = self.rate;
        let sounds = &self.sounds;
        self.voices.retain_mut(|v| {
            let s = &sounds[v.effect.index()];
            let step = f64::from(s.rate) / f64::from(rate);
            for frame in out.chunks_exact_mut(channels) {
                let i = v.at as usize;
                if i + 1 >= s.data.len() {
                    return false;
                }
                let t = (v.at - i as f64) as f32;
                let sample = s.data[i] * (1.0 - t) + s.data[i + 1] * t;
                if channels == 1 {
                    frame[0] += sample * (v.gain[0] + v.gain[1]) / 2.0;
                } else {
                    frame[0] += sample * v.gain[0];
                    frame[1] += sample * v.gain[1];
                }
                v.at += step;
            }
            true
        });
        for s in out {
            *s = s.clamp(-1.0, 1.0);
        }
    }
}

/// The sound output: opened on first use (on a thread of its own, so a sound server that
/// is slow to answer never holds up the GUI), kept open.
#[derive(Default)]
pub struct Sound {
    state: State,
}

#[derive(Default)]
enum State {
    /// Not tried yet.
    #[default]
    Closed,
    /// The output thread is opening it, until the deadline.
    Opening {
        opened: Receiver<Opened>,
        stop: Sender<()>,
        deadline: Instant,
    },
    Open(Output),
    /// Why there is no sound.
    Failed(String),
}

/// An open output: the mixer it plays.
struct Output {
    mixer: Arc<Mutex<Mixer>>,
    /// Dropped: the output thread closes the stream.
    _stop: Sender<()>,
}

/// How long opening the output may take.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

impl Sound {
    /// Plays the cues at `volume` (0 to 1); at most each effect's voices of each, the
    /// latest. Opens the output first; until it is open, cues are dropped.
    pub fn play(&mut self, cues: &[Cue], volume: f32) {
        if cues.is_empty() {
            return;
        }
        let Some(output) = self.open() else {
            return;
        };
        let Ok(mut mixer) = output.mixer.lock() else {
            return;
        };
        let mut counts = [0usize; Effect::ALL.len()];
        for cue in cues.iter().rev() {
            let n = &mut counts[cue.effect.index()];
            if *n < cue.effect.voices() {
                *n += 1;
                mixer.play(*cue, volume);
            }
        }
    }

    /// The output once it is open: starts opening it the first time, and takes the
    /// output thread's answer without waiting for it. Says on stderr what it opened, or
    /// why not.
    fn open(&mut self) -> Option<&Output> {
        if matches!(self.state, State::Closed) {
            self.state = match start() {
                Ok((opened, stop)) => State::Opening {
                    opened,
                    stop,
                    deadline: Instant::now() + OPEN_TIMEOUT,
                },
                Err(e) => State::Failed(e),
            };
        }
        if let State::Opening {
            opened, deadline, ..
        } = &self.state
        {
            let answer = match opened.try_recv() {
                Ok(answer) => Some(answer),
                Err(TryRecvError::Empty) if Instant::now() < *deadline => None,
                Err(TryRecvError::Empty) => {
                    Some(Err("the audio output didn't open in time".to_string()))
                }
                Err(TryRecvError::Disconnected) => {
                    Some(Err("the audio output thread ended".to_string()))
                }
            };
            if let Some(answer) = answer {
                let State::Opening { stop, .. } = std::mem::take(&mut self.state) else {
                    unreachable!()
                };
                self.state = match answer {
                    Ok((mixer, name)) => {
                        eprintln!("robocode-s-gui: sound through {name}");
                        State::Open(Output { mixer, _stop: stop })
                    }
                    Err(e) => {
                        eprintln!("robocode-s-gui: no sound: {e}");
                        State::Failed(e)
                    }
                };
            }
        }
        match &self.state {
            State::Open(output) => Some(output),
            _ => None,
        }
    }

    /// Starts opening the output now (sound was switched on), so a failure shows in the
    /// options. True while it is still opening (to look again soon).
    pub fn prepare(&mut self) -> bool {
        let _ = self.open();
        matches!(self.state, State::Opening { .. })
    }

    /// Why there is no sound, once it was tried.
    pub fn error(&self) -> Option<&str> {
        match &self.state {
            State::Failed(e) => Some(e),
            _ => None,
        }
    }
}

/// Starts the output thread, which opens the output and keeps it until the stop sender
/// drops. Returns where its answer comes (the mixer and a description of the device, or
/// why not) and the stop sender.
fn start() -> Result<(Receiver<Opened>, Sender<()>), String> {
    let (stop, stopped) = mpsc::channel::<()>();
    let (ready, opened) = mpsc::channel();
    std::thread::Builder::new()
        .name("sound".into())
        .spawn(move || backend::run(ready, stopped))
        .map_err(|e| e.to_string())?;
    Ok((opened, stop))
}

/// What the output thread reports: the mixer and the device's name, or why not.
type Opened = Result<(Arc<Mutex<Mixer>>, String), String>;

#[cfg(all(unix, not(target_os = "macos")))]
mod backend {
    //! PulseAudio's native protocol (served by PulseAudio and PipeWire), spoken
    //! synchronously on the output thread: the server asks for bytes, the mixer fills
    //! them.
    use super::{Mixer, Opened};
    use pulseaudio::protocol::{self, Command};
    use std::io::BufReader;
    use std::os::unix::net::UnixStream;
    use std::sync::mpsc::{Receiver, Sender, TryRecvError};
    use std::sync::{Arc, Mutex};

    const RATE: u32 = 44_100;
    /// Stereo 16-bit frames.
    const FRAME: usize = 4;

    pub fn run(ready: Sender<Opened>, stop: Receiver<()>) {
        let (mut socket, version, reply, name) = match connect() {
            Ok(connected) => connected,
            Err(e) => {
                let _ = ready.send(Err(format!("PulseAudio: {e}")));
                return;
            }
        };
        let mixer = Arc::new(Mutex::new(Mixer::new(RATE)));
        let _ = ready.send(Ok((Arc::clone(&mixer), name)));
        let channel = reply.channel;
        let mut frames = Vec::new();
        let mut bytes = Vec::new();
        let mut send = |socket: &mut BufReader<UnixStream>, length: usize| {
            let n = length / FRAME * FRAME;
            frames.resize(n / 2, 0.0f32);
            match mixer.lock() {
                Ok(mut m) => m.fill(&mut frames, 2),
                Err(_) => frames.fill(0.0),
            }
            bytes.clear();
            for s in &frames {
                bytes.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
            }
            protocol::write_memblock(socket.get_mut(), channel, &bytes, 0)
        };
        let mut result =
            send(&mut socket, reply.requested_bytes as usize).map_err(|e| e.to_string());
        // Until the GUI drops its end (checked whenever the server asks for more).
        while result.is_ok() && matches!(stop.try_recv(), Err(TryRecvError::Empty)) {
            result = match protocol::read_command_message(&mut socket, version) {
                Ok((_, Command::Request(r))) if r.channel == channel => {
                    send(&mut socket, r.length as usize).map_err(|e| e.to_string())
                }
                Ok((_, Command::Error(e))) => Err(format!("{e:?}")),
                Ok((_, Command::PlaybackStreamKilled(_))) => Err("the stream was closed".into()),
                Ok(_) => Ok(()),
                Err(e) => Err(e.to_string()),
            };
        }
        if let Err(e) = result {
            eprintln!("robocode-s-gui: sound stopped: PulseAudio: {e}");
        }
    }

    /// Connects, and creates a playback stream of about 60 ms: the effects keep up with
    /// the battlefield.
    fn connect() -> Result<
        (
            BufReader<UnixStream>,
            u16,
            protocol::CreatePlaybackStreamReply,
            String,
        ),
        String,
    > {
        let path = pulseaudio::socket_path_from_env().ok_or("no server found")?;
        let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let mut socket = BufReader::new(UnixStream::connect(&path).map_err(|e| fail(&e))?);
        // The server wants a cookie of 256 bytes: zeros without a cookie file, as libpulse
        // sends (WSLg's server has none).
        let mut cookie = pulseaudio::cookie_path_from_env()
            .and_then(|p| std::fs::read(p).ok())
            .unwrap_or_default();
        cookie.resize(256, 0);
        let auth = protocol::AuthParams {
            version: protocol::MAX_VERSION,
            supports_shm: false,
            supports_memfd: false,
            cookie,
        };
        let max = protocol::MAX_VERSION;
        protocol::write_command_message(socket.get_mut(), 0, &Command::Auth(auth), max)
            .map_err(|e| fail(&e))?;
        let (_, auth) = protocol::read_reply_message::<protocol::AuthReply>(&mut socket, max)
            .map_err(|e| fail(&e))?;
        let version = max.min(auth.version);
        let mut props = protocol::Props::new();
        props.set(protocol::Prop::ApplicationName, c"robocode-s");
        let name = Command::SetClientName(props);
        protocol::write_command_message(socket.get_mut(), 1, &name, version)
            .map_err(|e| fail(&e))?;
        protocol::read_reply_message::<protocol::SetClientNameReply>(&mut socket, version)
            .map_err(|e| fail(&e))?;
        let bytes = (RATE as usize * FRAME * 60 / 1000) as u32;
        let params = protocol::PlaybackStreamParams {
            sample_spec: protocol::SampleSpec {
                format: protocol::SampleFormat::S16Le,
                channels: 2,
                sample_rate: RATE,
            },
            channel_map: protocol::ChannelMap::stereo(),
            cvolume: Some(protocol::ChannelVolume::norm(2)),
            sink_name: Some(protocol::DEFAULT_SINK.to_owned()),
            buffer_attr: protocol::stream::BufferAttr {
                target_length: bytes,
                minimum_request_length: bytes / 4,
                ..Default::default()
            },
            ..Default::default()
        };
        let create = Command::CreatePlaybackStream(params);
        protocol::write_command_message(socket.get_mut(), 2, &create, version)
            .map_err(|e| fail(&e))?;
        let (_, reply) = protocol::read_reply_message::<protocol::CreatePlaybackStreamReply>(
            &mut socket,
            version,
        )
        .map_err(|e| fail(&e))?;
        let name = format!(
            "PulseAudio at {} (sink {}, {} ms)",
            path.display(),
            reply
                .sink_name
                .as_ref()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
            reply.stream_latency / 1000
        );
        Ok((socket, version, reply, name))
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod backend {
    //! cpal: WASAPI on Windows, Core Audio on macOS.
    use super::{Mixer, Opened};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::mpsc::{Receiver, Sender};
    use std::sync::{Arc, Mutex};

    pub fn run(ready: Sender<Opened>, stop: Receiver<()>) {
        match open() {
            Ok((stream, mixer, name)) => {
                let _ = ready.send(Ok((mixer, name)));
                let _ = stop.recv();
                drop(stream);
            }
            Err(e) => {
                let _ = ready.send(Err(e));
            }
        }
    }

    fn open() -> Result<(cpal::Stream, Arc<Mutex<Mixer>>, String), String> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("no audio output device")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let config = supported.config();
        let channels = usize::from(config.channels);
        let mixer = Arc::new(Mutex::new(Mixer::new(config.sample_rate)));
        let fill = Arc::clone(&mixer);
        let error = |e| eprintln!("robocode-s-gui: sound: {e}");
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| match fill.lock() {
                    Ok(mut m) => m.fill(out, channels),
                    Err(_) => out.fill(0.0),
                },
                error,
                None,
            ),
            cpal::SampleFormat::I16 => {
                let mut frames = Vec::new();
                device.build_output_stream(
                    config,
                    move |out: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        frames.resize(out.len(), 0.0f32);
                        match fill.lock() {
                            Ok(mut m) => m.fill(&mut frames, channels),
                            Err(_) => frames.fill(0.0),
                        }
                        for (o, s) in out.iter_mut().zip(&frames) {
                            *o = (s * 32767.0) as i16;
                        }
                    },
                    error,
                    None,
                )
            }
            other => return Err(format!("unsupported sample format {other}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok((stream, mixer, "the default output device".into()))
    }
}

#[cfg(not(any(unix, windows)))]
mod backend {
    use super::Opened;
    use std::sync::mpsc::{Receiver, Sender};

    pub fn run(ready: Sender<Opened>, _stop: Receiver<()>) {
        let _ = ready.send(Err("no audio output on this platform".into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bullet(state: v1::BulletState, frame: i32, x: f64, power: f64) -> v1::BulletSnapshot {
        v1::BulletSnapshot {
            state: state as i32,
            frame,
            x: Some(x),
            power: Some(power),
            ..Default::default()
        }
    }

    fn robot(state: v1::RobotState, x: f64) -> v1::RobotSnapshot {
        v1::RobotSnapshot {
            state: state as i32,
            x: Some(x),
            ..Default::default()
        }
    }

    #[test]
    fn turns_map_to_classic_effects() {
        use v1::BulletState as B;
        use v1::RobotState as R;
        let turn = v1::TurnSnapshot {
            bullets: vec![
                bullet(B::Fired, 0, 0.0, 3.0),
                bullet(B::Fired, 0, 400.0, 0.3),
                bullet(B::Moving, 0, 400.0, 3.0),
                bullet(B::HitVictim, 0, 800.0, 1.0),
                // Later frames of an explosion: no sound again.
                bullet(B::HitVictim, 3, 800.0, 1.0),
                bullet(B::HitBullet, 0, 600.0, 1.0),
                // Classic is silent when a bullet hits a wall.
                bullet(B::HitWall, 0, 600.0, 1.0),
                bullet(B::Exploded, 0, 200.0, 1.0),
                bullet(B::Inactive, 0, 200.0, 1.0),
            ],
            robots: vec![
                robot(R::HitRobot, 100.0),
                robot(R::HitRobot, 120.0),
                robot(R::HitWall, 800.0),
                robot(R::Active, 300.0),
                robot(R::Dead, 300.0),
            ],
            ..Default::default()
        };
        let got: Vec<(Effect, f32, f32)> = cues(&turn, 800.0)
            .into_iter()
            .map(|c| (c.effect, (c.volume * 100.0).round() / 100.0, c.pan))
            .collect();
        assert_eq!(
            got,
            vec![
                (Effect::Gunshot, 1.0, -1.0),
                (Effect::Gunshot, 0.1, 0.0),
                (Effect::BulletHitsRobot, 1.0, 1.0),
                (Effect::BulletHitsBullet, 1.0, 0.5),
                (Effect::RobotDeath, 1.0, -0.5),
                (Effect::RobotCollision, 1.0, -0.75),
                (Effect::WallCollision, 1.0, 1.0),
            ]
        );
    }

    #[test]
    fn classic_sounds_decode() {
        for effect in Effect::ALL {
            let s = decode_wav(effect.wav()).unwrap();
            assert!(s.data.len() > 1000, "{effect:?}");
            assert!(s.data.iter().all(|v| (-1.0..=1.0).contains(v)));
        }
        // explode.wav is 8-bit at 8 kHz, the others 16-bit.
        assert_eq!(decode_wav(Effect::RobotDeath.wav()).unwrap().rate, 8000);
        assert!(decode_wav(b"RIFF\0\0\0\0WAVE").is_err());
    }

    #[test]
    fn an_output_that_does_not_answer_never_holds_the_gui_up() {
        // An output thread that never answers, as a sound server that accepts and then
        // says nothing.
        let (_ready, opened) = mpsc::channel();
        let (stop, _stopped) = mpsc::channel();
        let mut sound = Sound {
            state: State::Opening {
                opened,
                stop,
                deadline: Instant::now() + Duration::from_millis(200),
            },
        };
        let cue = Cue {
            effect: Effect::Gunshot,
            volume: 1.0,
            pan: 0.0,
        };
        let begin = Instant::now();
        sound.play(&[cue], 1.0);
        assert!(sound.prepare(), "still opening");
        assert!(begin.elapsed() < Duration::from_millis(100), "no waiting");
        assert_eq!(sound.error(), None);
        std::thread::sleep(Duration::from_millis(250));
        assert!(!sound.prepare());
        assert!(sound.error().unwrap().contains("in time"));
    }

    #[test]
    fn the_mixer_limits_voices_and_pans() {
        let mut m = Mixer::new(44_100);
        let cue = |effect, pan| Cue {
            effect,
            volume: 1.0,
            pan,
        };
        for _ in 0..8 {
            m.play(cue(Effect::Gunshot, 0.0), 1.0);
        }
        m.play(cue(Effect::RobotDeath, -1.0), 1.0);
        assert_eq!(m.voices.len(), Effect::Gunshot.voices() + 1);
        // The oldest gunshots were restarted.
        assert!(m.voices.iter().all(|v| v.started > 3));
        assert_eq!(m.voices.last().unwrap().gain, [1.0, 0.0]);
        let mut out = vec![0.0; 2 * 4410];
        m.fill(&mut out, 2);
        assert!(out.iter().any(|&s| s != 0.0));
        assert!(out.iter().all(|s| (-1.0..=1.0).contains(s)));
        // Played to the end, the voices stop.
        let mut long = vec![0.0; 2 * 44_100 * 3];
        m.fill(&mut long, 2);
        assert!(m.voices.is_empty());
        // Quieter for less power, as classic: -20 dB at volume 0.
        assert!((amplitude(0.0) - 0.1).abs() < 1e-6);
        assert_eq!(amplitude(1.0), 1.0);
    }
}
