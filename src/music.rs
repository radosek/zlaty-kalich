//! MIDI music, replacing the original `player.exe` helper.
//!
//! The helper opened the `.mid` in a TMediaPlayer and on every Notify rewound
//! and replayed it, so each track loops until the next command.

use anyhow::{Context, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use rustysynth::{MidiFile, MidiFileSequencer, SoundFont, Synthesizer, SynthesizerSettings};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

static SOUNDFONT: &[u8] = include_bytes!("../assets/GeneralUser-GS.sf2");

struct State {
    sequencer: Option<MidiFileSequencer>,
    current: Option<PathBuf>,
    /// 0.0–1.0.
    volume: f32,
}

pub struct Music {
    state: Arc<Mutex<State>>,
    sample_rate: u32,
    soundfont: Arc<OnceLock<Arc<SoundFont>>>,
    _stream: cpal::Stream,
}

impl Music {
    /// `custom_soundfont` replaces the bundled GeneralUser GS bank.
    pub fn new(custom_soundfont: Option<PathBuf>, volume: f32) -> anyhow::Result<Self> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| anyhow!("no audio output device"))?;
        let config = device.default_output_config()?;
        let sample_rate = config.sample_rate().0;
        let state = Arc::new(Mutex::new(State {
            sequencer: None,
            current: None,
            volume,
        }));

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, &config.into(), state.clone()),
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, &config.into(), state.clone()),
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, &config.into(), state.clone()),
            cpal::SampleFormat::I32 => build_stream::<i32>(&device, &config.into(), state.clone()),
            format => return Err(anyhow!("unsupported sample format {format}")),
        }?;
        stream.play()?;

        // Parsing the 32 MB bank takes a moment; do it off the UI thread.
        let soundfont = Arc::new(OnceLock::new());
        let slot = soundfont.clone();
        std::thread::spawn(move || {
            let custom = custom_soundfont.and_then(|path| match std::fs::read(&path) {
                Ok(bytes) => match SoundFont::new(&mut Cursor::new(bytes)) {
                    Ok(sf) => Some(sf),
                    Err(e) => {
                        tracing::error!("{}: {e}; using the bundled soundfont", path.display());
                        None
                    }
                },
                Err(e) => {
                    tracing::error!("{}: {e}; using the bundled soundfont", path.display());
                    None
                }
            });
            match custom.map_or_else(|| SoundFont::new(&mut Cursor::new(SOUNDFONT)), Ok) {
                Ok(sf) => {
                    let _ = slot.set(Arc::new(sf));
                }
                Err(e) => tracing::error!("Couldn't load soundfont: {e}"),
            }
        });

        Ok(Self {
            state,
            sample_rate,
            soundfont,
            _stream: stream,
        })
    }

    pub fn play(&self, path: &Path) -> anyhow::Result<()> {
        if self.state.lock().unwrap().current.as_deref() == Some(path) {
            return Ok(());
        }
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let midi = Arc::new(
            MidiFile::new(&mut Cursor::new(bytes))
                .map_err(|e| anyhow!("{}: {e}", path.display()))?,
        );
        let soundfont = self.soundfont.wait().clone();
        let settings = SynthesizerSettings::new(self.sample_rate as i32);
        let synth = Synthesizer::new(&soundfont, &settings).map_err(|e| anyhow!("{e}"))?;
        let mut sequencer = MidiFileSequencer::new(synth);
        sequencer.play(&midi, true);

        let mut state = self.state.lock().unwrap();
        state.sequencer = Some(sequencer);
        state.current = Some(path.to_owned());
        Ok(())
    }

    pub fn set_volume(&self, volume: f32) {
        self.state.lock().unwrap().volume = volume.clamp(0.0, 1.0);
    }

    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        state.sequencer = None;
        state.current = None;
    }
}

fn build_stream<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    state: Arc<Mutex<State>>,
) -> anyhow::Result<cpal::Stream> {
    let channels = config.channels as usize;
    let mut left = Vec::new();
    let mut right = Vec::new();
    let stream = device.build_output_stream(
        config,
        move |out: &mut [T], _| {
            let frames = out.len() / channels;
            left.resize(frames, 0.0);
            right.resize(frames, 0.0);
            let mut state = state.lock().unwrap();
            let volume = state.volume;
            match state.sequencer.as_mut() {
                Some(sequencer) => sequencer.render(&mut left, &mut right),
                None => {
                    left.fill(0.0);
                    right.fill(0.0);
                }
            }
            drop(state);
            for (i, frame) in out.chunks_mut(channels).enumerate() {
                for (c, sample) in frame.iter_mut().enumerate() {
                    let v = volume * if c % 2 == 0 { left[i] } else { right[i] };
                    *sample = T::from_sample(v);
                }
            }
        },
        |e| tracing::error!("Music stream error: {e}"),
        None,
    )?;
    Ok(stream)
}
