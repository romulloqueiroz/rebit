//! Cadence native-protocol wrapper for the Rebit Rust engine.
//!
//! Build this crate as a `cdylib` and open the resulting `.dylib` with
//! Cadence's **Open...** button. The bridge loads it in an isolated worker and
//! exposes the parameters below through Cadence's generic inspector.
//!
//! The engine is the same code used by Rebit's JUCE AU/VST3/Standalone builds:
//! two independent voices engines ("layers") summed by the stereo mixer with
//! its shared colour effects. Sequences and Rebit's factory preset browser are
//! not part of the native protocol surface yet.

use plugin_api::{
    AudioBuffer, Capabilities, Descriptor, Event, EventKind, ParamFlags, ParameterInfo, Plugin,
    PluginKind, ProcessContext, State, StateError, Unit,
};
use plugin_sdk::export_plugin;
use rebit_rust::effects::EffectsParameters;
use rebit_rust::layers::{LayerMix, LayerMixer};
use rebit_rust::{
    ArpParameters, ClockParameters, Engine, LfoParameters, ModRoute, ModulationParameters,
    Parameters, SequenceBank, TransportInfo,
};

const MAX_EVENTS: usize = 256;

const DESCRIPTOR: Descriptor = Descriptor {
    id: "com.rebit.rebit-rust",
    name: "Rebit Rust",
    vendor: "Rebit Audio",
    version: env!("CARGO_PKG_VERSION"),
    kind: PluginKind::Instrument,
};

const CAPABILITIES: Capabilities = Capabilities {
    inputs: 0,
    outputs: 2,
    sidechain_inputs: 0,
    latency_samples: 0,
    tail_samples: 5 * 48_000,
};

#[derive(Clone, Copy)]
#[repr(usize)]
enum Id {
    Gain,
    Osc,
    Duty,
    NoiseAlgorithm,
    MaxPoly,
    Behavior,
    Attack,
    Decay,
    Sustain,
    Release,
    Portamento,
    BendRange,
    VibratoPeriod,
    VibratoDepth,
    VibratoDelay,
    IgnoresModWheel,
    SweepInitial,
    SweepTime,
    IntervalType,
    IntervalSlider,
    PitchMode,
    VolumeSeq,
    PitchSeq,
    DutySeq,
    SoundMode,
    ArpEnabled,
    ArpLatch,
    ArpPattern,
    ArpRate,
    ArpOctaves,
    ArpRetrigger,
    ArpSeconds,
    ArpGate,
    ArpSwing,
    HostSync,
    SequenceClock,
    LayerAEnabled,
    LayerALevel,
    LayerAPan,
    LayerATranspose,
    LayerADetune,
    LayerBEnabled,
    LayerBLevel,
    LayerBPan,
    LayerBTranspose,
    LayerBDetune,
    LayerBWaveform,
    LayerBDuty,
    LayerBNoise,
    LayerBAttack,
    LayerBDecay,
    LayerBSustain,
    LayerBRelease,
    LayerBVolumeSeq,
    LayerBPitchSeq,
    LayerBDutySeq,
    LayerBPitchMode,
    LayerASoundMode,
    LayerBSoundMode,
    FxEnabled,
    FxDrive,
    FxBits,
    FxRate,
    FxMix,
    Macro1,
    Macro2,
    Macro3,
    Macro4,
    Mod1Source,
    Mod1Dest,
    Mod1Amount,
    Mod2Source,
    Mod2Dest,
    Mod2Amount,
    Mod3Source,
    Mod3Dest,
    Mod3Amount,
    Mod4Source,
    Mod4Dest,
    Mod4Amount,
    Lfo1Shape,
    Lfo1Rate,
    Lfo1Retrigger,
    Lfo2Shape,
    Lfo2Rate,
    Lfo2Retrigger,
    MpeEnabled,
    MpeBendRange,
}

impl Id {
    const fn id(self) -> u32 {
        self as u32 + 1
    }
    const fn index(self) -> usize {
        self as usize
    }
}

const PARAM_COUNT: usize = Id::MpeBendRange as usize + 1;

const fn p(
    id: u32,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    stepped: bool,
) -> ParameterInfo {
    ParameterInfo {
        id,
        name,
        min,
        max,
        default,
        unit: Unit::None,
        flags: ParamFlags {
            stepped,
            read_only: false,
            bypass: false,
        },
    }
}

macro_rules! param {
    ($id:ident, $name:literal, $min:expr, $max:expr, $default:expr, $stepped:expr) => {
        p(Id::$id.id(), $name, $min, $max, $default, $stepped)
    };
}

const PARAMS: [ParameterInfo; PARAM_COUNT] = [
    param!(Gain, "Gain", 0.0, 1.0, 0.5, false),
    param!(Osc, "OSC Type", 0.0, 2.0, 0.0, true),
    param!(Duty, "Duty", 0.0, 2.0, 0.0, true),
    param!(NoiseAlgorithm, "Noise Algorithm", 0.0, 2.0, 0.0, true),
    param!(MaxPoly, "Max Poly", 1.0, 64.0, 8.0, true),
    param!(Behavior, "Behavior", 0.0, 3.0, 0.0, true),
    param!(Attack, "Attack", 0.0, 5.0, 0.0, false),
    param!(Decay, "Decay", 0.0, 5.0, 0.0, false),
    param!(Sustain, "Sustain", 0.0, 1.0, 1.0, false),
    param!(Release, "Release", 0.0, 5.0, 0.0, false),
    param!(Portamento, "Portamento", 0.0, 1.0, 0.0, false),
    param!(BendRange, "Bend Range", 0.0, 24.0, 2.0, true),
    param!(VibratoPeriod, "Vibrato Period", 0.01, 1.0, 0.15, false),
    param!(VibratoDepth, "Vibrato Depth", 0.0, 2.0, 0.0, false),
    param!(VibratoDelay, "Vibrato Delay", 0.0, 1.0, 0.3, false),
    param!(IgnoresModWheel, "Ignores Mod Wheel", 0.0, 1.0, 1.0, true),
    param!(SweepInitial, "Sweep Initial Pitch", -24.0, 24.0, 0.0, true),
    param!(SweepTime, "Sweep Time", 0.01, 5.0, 0.1, false),
    param!(IntervalType, "Arp Interval Type", 0.0, 8.0, 0.0, true),
    param!(IntervalSlider, "Arp Interval Seconds", 0.001, 0.3, 0.1, false),
    param!(PitchMode, "Pitch Sequence Mode", 0.0, 1.0, 0.0, true),
    param!(VolumeSeq, "Volume Sequence", 0.0, 1.0, 0.0, true),
    param!(PitchSeq, "Pitch Sequence", 0.0, 1.0, 0.0, true),
    param!(DutySeq, "Duty Sequence", 0.0, 1.0, 0.0, true),
    param!(SoundMode, "Sound Mode", 0.0, 1.0, 0.0, true),
    param!(ArpEnabled, "Arp Enabled", 0.0, 1.0, 0.0, true),
    param!(ArpLatch, "Arp Latch", 0.0, 1.0, 0.0, true),
    param!(ArpPattern, "Arp Pattern", 0.0, 4.0, 0.0, true),
    param!(ArpRate, "Arp Rate", 0.0, 15.0, 2.0, true),
    param!(ArpOctaves, "Arp Octaves", 1.0, 4.0, 1.0, true),
    param!(ArpRetrigger, "Arp Retrigger", 0.0, 1.0, 1.0, true),
    param!(ArpSeconds, "Arp Step Seconds", 0.01, 2.0, 0.125, false),
    param!(ArpGate, "Arp Gate", 5.0, 100.0, 65.0, false),
    param!(ArpSwing, "Arp Swing", 0.0, 49.0, 0.0, false),
    param!(HostSync, "Host Sync", 0.0, 1.0, 1.0, true),
    param!(SequenceClock, "Sequence Clock", 0.0, 7.0, 0.0, true),
    param!(LayerAEnabled, "Layer A Enabled", 0.0, 1.0, 1.0, true),
    param!(LayerALevel, "Layer A Level", 0.0, 1.0, 1.0, false),
    param!(LayerAPan, "Layer A Pan", -1.0, 1.0, 0.0, false),
    param!(LayerATranspose, "Layer A Transpose", -24.0, 24.0, 0.0, true),
    param!(LayerADetune, "Layer A Detune", -100.0, 100.0, 0.0, false),
    param!(LayerBEnabled, "Layer B Enabled", 0.0, 1.0, 0.0, true),
    param!(LayerBLevel, "Layer B Level", 0.0, 1.0, 0.5, false),
    param!(LayerBPan, "Layer B Pan", -1.0, 1.0, 0.0, false),
    param!(LayerBTranspose, "Layer B Transpose", -24.0, 24.0, -12.0, true),
    param!(LayerBDetune, "Layer B Detune", -100.0, 100.0, 0.0, false),
    param!(LayerBWaveform, "Layer B Waveform", 0.0, 2.0, 1.0, true),
    param!(LayerBDuty, "Layer B Duty", 0.0, 2.0, 0.0, true),
    param!(LayerBNoise, "Layer B Noise", 0.0, 2.0, 0.0, true),
    param!(LayerBAttack, "Layer B Attack", 0.0, 5.0, 0.0, false),
    param!(LayerBDecay, "Layer B Decay", 0.0, 5.0, 0.0, false),
    param!(LayerBSustain, "Layer B Sustain", 0.0, 1.0, 1.0, false),
    param!(LayerBRelease, "Layer B Release", 0.0, 5.0, 0.0, false),
    param!(LayerBVolumeSeq, "Layer B Volume Sequence", 0.0, 1.0, 0.0, true),
    param!(LayerBPitchSeq, "Layer B Pitch Sequence", 0.0, 1.0, 0.0, true),
    param!(LayerBDutySeq, "Layer B Duty Sequence", 0.0, 1.0, 0.0, true),
    param!(LayerBPitchMode, "Layer B Pitch Units", 0.0, 1.0, 0.0, true),
    param!(LayerASoundMode, "Layer A Sound Mode", 0.0, 1.0, 0.0, true),
    param!(LayerBSoundMode, "Layer B Sound Mode", 0.0, 1.0, 0.0, true),
    param!(FxEnabled, "Effects Enabled", 0.0, 1.0, 0.0, true),
    param!(FxDrive, "Saturation Drive", 0.0, 24.0, 0.0, false),
    param!(FxBits, "Bit Depth", 4.0, 24.0, 24.0, true),
    param!(FxRate, "Rate Divider", 1.0, 32.0, 1.0, true),
    param!(FxMix, "Effects Mix", 0.0, 100.0, 100.0, false),
    param!(Macro1, "Macro 1", 0.0, 1.0, 0.0, false),
    param!(Macro2, "Macro 2", 0.0, 1.0, 0.0, false),
    param!(Macro3, "Macro 3", 0.0, 1.0, 0.0, false),
    param!(Macro4, "Macro 4", 0.0, 1.0, 0.0, false),
    param!(Mod1Source, "Mod 1 Source", 0.0, 12.0, 0.0, true),
    param!(Mod1Dest, "Mod 1 Destination", 0.0, 6.0, 0.0, true),
    param!(Mod1Amount, "Mod 1 Amount", -100.0, 100.0, 0.0, true),
    param!(Mod2Source, "Mod 2 Source", 0.0, 12.0, 0.0, true),
    param!(Mod2Dest, "Mod 2 Destination", 0.0, 6.0, 0.0, true),
    param!(Mod2Amount, "Mod 2 Amount", -100.0, 100.0, 0.0, true),
    param!(Mod3Source, "Mod 3 Source", 0.0, 12.0, 0.0, true),
    param!(Mod3Dest, "Mod 3 Destination", 0.0, 6.0, 0.0, true),
    param!(Mod3Amount, "Mod 3 Amount", -100.0, 100.0, 0.0, true),
    param!(Mod4Source, "Mod 4 Source", 0.0, 12.0, 0.0, true),
    param!(Mod4Dest, "Mod 4 Destination", 0.0, 6.0, 0.0, true),
    param!(Mod4Amount, "Mod 4 Amount", -100.0, 100.0, 0.0, true),
    param!(Lfo1Shape, "LFO 1 Shape", 0.0, 5.0, 0.0, true),
    param!(Lfo1Rate, "LFO 1 Rate", 0.01, 20.0, 1.0, false),
    param!(Lfo1Retrigger, "LFO 1 Retrigger", 0.0, 1.0, 0.0, true),
    param!(Lfo2Shape, "LFO 2 Shape", 0.0, 5.0, 0.0, true),
    param!(Lfo2Rate, "LFO 2 Rate", 0.01, 20.0, 1.0, false),
    param!(Lfo2Retrigger, "LFO 2 Retrigger", 0.0, 1.0, 0.0, true),
    param!(MpeEnabled, "MPE Enabled", 0.0, 1.0, 0.0, true),
    param!(MpeBendRange, "MPE Bend Range", 0.0, 96.0, 48.0, true),
];

const MOD_ROUTES: [(Id, Id, Id); 4] = [
    (Id::Mod1Source, Id::Mod1Dest, Id::Mod1Amount),
    (Id::Mod2Source, Id::Mod2Dest, Id::Mod2Amount),
    (Id::Mod3Source, Id::Mod3Dest, Id::Mod3Amount),
    (Id::Mod4Source, Id::Mod4Dest, Id::Mod4Amount),
];
const MACROS: [Id; 4] = [Id::Macro1, Id::Macro2, Id::Macro3, Id::Macro4];
const LFOS: [(Id, Id, Id); 2] = [
    (Id::Lfo1Shape, Id::Lfo1Rate, Id::Lfo1Retrigger),
    (Id::Lfo2Shape, Id::Lfo2Rate, Id::Lfo2Retrigger),
];

fn default_norm() -> [f32; PARAM_COUNT] {
    let mut values = [0.0; PARAM_COUNT];
    for (index, parameter) in PARAMS.iter().enumerate() {
        values[index] = parameter.normalize(parameter.default);
    }
    values
}

fn key_for(parameter: &ParameterInfo) -> String {
    parameter
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

fn wire_index(id: u32) -> Option<usize> {
    if id >= 1 && (id as usize) <= PARAM_COUNT {
        Some(id as usize - 1)
    } else {
        None
    }
}

fn sort_events(events: &mut [Event]) {
    for i in 1..events.len() {
        let event = events[i];
        let mut j = i;
        while j > 0 && events[j - 1].frame > event.frame {
            events[j] = events[j - 1];
            j -= 1;
        }
        events[j] = event;
    }
}

fn u7(value: u16) -> u8 {
    value.min(127) as u8
}

struct RebitPlugin {
    engines: [Engine; 2],
    mixer: LayerMixer,
    norm: [f32; PARAM_COUNT],
}

impl Default for RebitPlugin {
    fn default() -> Self {
        let mut plugin = RebitPlugin {
            engines: [Engine::default(), Engine::new(44_100.0, 0xcc75_30ed)],
            mixer: LayerMixer::default(),
            norm: default_norm(),
        };
        plugin.engines[0].set_sequences(SequenceBank::default());
        plugin.engines[1].set_sequences(SequenceBank::default());
        plugin.apply();
        plugin
    }
}

impl RebitPlugin {
    fn value(&self, id: Id) -> f32 {
        PARAMS[id.index()].denormalize(self.norm[id.index()])
    }

    fn int(&self, id: Id) -> i32 {
        self.value(id).round() as i32
    }

    fn toggle(&self, id: Id) -> i32 {
        i32::from(self.value(id) >= 0.5)
    }

    fn parameters_a(&self) -> Parameters {
        Parameters {
            osc: self.int(Id::Osc),
            duty: self.int(Id::Duty),
            noise_algorithm: self.int(Id::NoiseAlgorithm),
            max_poly: self.int(Id::MaxPoly),
            behavior: self.int(Id::Behavior),
            interval_type: self.int(Id::IntervalType),
            pitch_mode: self.int(Id::PitchMode),
            gain: self.value(Id::Gain),
            attack: self.value(Id::Attack),
            decay: self.value(Id::Decay),
            sustain: self.value(Id::Sustain),
            release: self.value(Id::Release),
            portamento: self.value(Id::Portamento),
            bend_range: if self.toggle(Id::MpeEnabled) != 0 {
                self.value(Id::MpeBendRange)
            } else {
                self.value(Id::BendRange)
            },
            vibrato_period: self.value(Id::VibratoPeriod),
            vibrato_depth: self.value(Id::VibratoDepth),
            vibrato_delay: self.value(Id::VibratoDelay),
            sweep_initial: self.value(Id::SweepInitial),
            sweep_time: self.value(Id::SweepTime),
            interval: self.value(Id::IntervalSlider),
            ignores_wheel: self.toggle(Id::IgnoresModWheel),
            volume_enabled: self.toggle(Id::VolumeSeq),
            pitch_enabled: self.toggle(Id::PitchSeq),
            duty_enabled: self.toggle(Id::DutySeq),
        }
    }

    fn parameters_b(&self) -> Parameters {
        let mut parameters = self.parameters_a();
        parameters.osc = self.int(Id::LayerBWaveform);
        parameters.duty = self.int(Id::LayerBDuty);
        parameters.noise_algorithm = self.int(Id::LayerBNoise);
        parameters.attack = self.value(Id::LayerBAttack);
        parameters.decay = self.value(Id::LayerBDecay);
        parameters.sustain = self.value(Id::LayerBSustain);
        parameters.release = self.value(Id::LayerBRelease);
        parameters.volume_enabled = self.toggle(Id::LayerBVolumeSeq);
        parameters.pitch_enabled = self.toggle(Id::LayerBPitchSeq);
        parameters.duty_enabled = self.toggle(Id::LayerBDutySeq);
        parameters.pitch_mode = self.int(Id::LayerBPitchMode);
        parameters
    }

    fn modulation_parameters(&self) -> ModulationParameters {
        let mut parameters = ModulationParameters::default();
        for (slot, (source, destination, amount)) in MOD_ROUTES.iter().enumerate() {
            parameters.routes[slot] = ModRoute {
                source: self.int(*source),
                destination: self.int(*destination),
                amount: self.value(*amount) * 0.01,
            };
        }
        for (slot, id) in MACROS.iter().enumerate() {
            parameters.macros[slot] = self.value(*id);
        }
        for (slot, (shape, rate, retrigger)) in LFOS.iter().enumerate() {
            parameters.lfo[slot] = LfoParameters {
                shape: self.int(*shape),
                rate_hz: self.value(*rate),
                retrigger: self.toggle(*retrigger),
            };
        }
        parameters
    }

    fn arp_parameters(&self) -> ArpParameters {
        ArpParameters {
            enabled: self.toggle(Id::ArpEnabled),
            pattern: self.int(Id::ArpPattern),
            rate: self.int(Id::ArpRate),
            octaves: self.int(Id::ArpOctaves),
            latch: self.toggle(Id::ArpLatch),
            retrigger: self.toggle(Id::ArpRetrigger),
            seconds: self.value(Id::ArpSeconds),
            gate: self.value(Id::ArpGate) * 0.01,
            swing: self.value(Id::ArpSwing) * 0.01,
        }
    }

    fn clock_parameters(&self) -> ClockParameters {
        ClockParameters {
            host_sync: self.toggle(Id::HostSync),
            sequence_rate: self.int(Id::SequenceClock),
        }
    }

    fn apply(&mut self) {
        let parameters_a = self.parameters_a();
        let parameters_b = self.parameters_b();
        let modulation = self.modulation_parameters();
        let arp = self.arp_parameters();
        let clock = self.clock_parameters();

        self.engines[0].set_parameters(parameters_a);
        self.engines[1].set_parameters(parameters_b);
        for engine in &mut self.engines {
            engine.set_modulation(modulation);
            engine.set_arp_parameters(arp);
            engine.set_clock_parameters(clock);
        }
        self.engines[0].set_sound_mode(self.toggle(Id::LayerASoundMode));
        self.engines[1].set_sound_mode(self.toggle(Id::LayerBSoundMode));
        self.engines[0].set_tuning(
            f64::from(self.value(Id::LayerATranspose))
                + f64::from(self.value(Id::LayerADetune)) * 0.01,
        );
        self.engines[1].set_tuning(
            f64::from(self.value(Id::LayerBTranspose))
                + f64::from(self.value(Id::LayerBDetune)) * 0.01,
        );
        self.mixer.set_mix(
            0,
            LayerMix {
                enabled: self.toggle(Id::LayerAEnabled),
                gain: self.value(Id::LayerALevel),
                pan: self.value(Id::LayerAPan),
            },
        );
        self.mixer.set_mix(
            1,
            LayerMix {
                enabled: self.toggle(Id::LayerBEnabled),
                gain: self.value(Id::LayerBLevel),
                pan: self.value(Id::LayerBPan),
            },
        );
        self.mixer.set_effects(EffectsParameters {
            enabled: self.toggle(Id::FxEnabled),
            drive_db: self.value(Id::FxDrive),
            bits: self.value(Id::FxBits),
            rate_divider: self.value(Id::FxRate),
            mix: self.value(Id::FxMix) * 0.01,
        });
    }

    fn update_host(&mut self, context: &ProcessContext) {
        let clock = self.clock_parameters();
        let transport = TransportInfo {
            ppq: context.ppq_position,
            bpm: context.tempo,
            loop_start: 0.0,
            loop_end: 0.0,
            valid: i32::from(
                context.tempo.is_finite()
                    && context.tempo > 0.0
                    && context.ppq_position.is_finite(),
            ),
            playing: i32::from(context.playing),
            looping: 0,
            reserved: 0,
        };
        for engine in &mut self.engines {
            engine.set_clock_parameters(clock);
            engine.set_transport(transport);
        }
    }

    fn send_midi(&mut self, event: &Event) {
        let channel = event.channel & 0x0f;
        match event.kind {
            EventKind::NoteOn if event.data2 == 0 => {
                self.midi_all(0x80 | channel, u7(event.data1), 0);
            }
            EventKind::NoteOn => {
                self.midi_all(0x90 | channel, u7(event.data1), u7(event.data2));
            }
            EventKind::NoteOff => {
                self.midi_all(0x80 | channel, u7(event.data1), u7(event.data2));
            }
            EventKind::PolyPressure => {
                self.midi_all(0xa0 | channel, u7(event.data1), u7(event.data2));
            }
            EventKind::ControlChange => {
                if event.data1 == 120 {
                    self.mixer.clear_effects_history();
                }
                self.midi_all(0xb0 | channel, u7(event.data1), u7(event.data2));
            }
            EventKind::ProgramChange => {
                self.midi_all(0xc0 | channel, u7(event.data1), 0);
            }
            EventKind::PitchBend => {
                let bend = ((event.value + 1.0) * 8192.0)
                    .round()
                    .clamp(0.0, 16_383.0) as u16;
                self.midi_all(0xe0 | channel, (bend & 0x7f) as u8, (bend >> 7) as u8);
            }
            EventKind::ChannelPressure => {
                self.midi_all(0xd0 | channel, u7(event.data1), 0);
            }
            EventKind::Parameter => {}
        }
    }

    fn midi_all(&mut self, status: u8, data1: u8, data2: u8) {
        for engine in &mut self.engines {
            engine.midi(status, data1, data2);
        }
    }

    fn render_stereo(&mut self, left: &mut [f32], right: &mut [f32], from: usize, to: usize) {
        if to <= from {
            return;
        }
        let Self { mixer, engines, .. } = self;
        let [first, second] = engines;
        mixer.render(first, second, &mut left[from..to], &mut right[from..to]);
    }

    fn render_mono(&mut self, output: &mut [f32], from: usize, to: usize) {
        if to <= from {
            return;
        }
        let Self { mixer, engines, .. } = self;
        let [first, second] = engines;
        mixer.render_mono(first, second, &mut output[from..to]);
    }
}

impl Plugin for RebitPlugin {
    fn descriptor(&self) -> Descriptor {
        DESCRIPTOR
    }

    fn capabilities(&self) -> Capabilities {
        CAPABILITIES
    }

    fn parameters(&self) -> Vec<ParameterInfo> {
        PARAMS.to_vec()
    }

    fn parameter_key(&self, id: u32) -> String {
        id.checked_sub(1)
            .and_then(|index| PARAMS.get(index as usize))
            .map(key_for)
            .unwrap_or_else(|| id.to_string())
    }

    fn prepare(&mut self, context: &ProcessContext) {
        for engine in &mut self.engines {
            engine.prepare(context.sample_rate);
        }
        self.mixer.prepare(context.sample_rate);
        self.apply();
    }

    fn reset(&mut self) {
        for engine in &mut self.engines {
            engine.reset();
        }
        self.mixer.reset();
        self.apply();
    }

    fn process(
        &mut self,
        context: &ProcessContext,
        audio: &mut AudioBuffer<'_>,
        events: &[Event],
    ) {
        let frames = audio.frames;
        if frames == 0 || audio.output_channels == 0 {
            return;
        }

        let mut sorted = [Event::note_off(0, 0); MAX_EVENTS];
        let count = events.len().min(MAX_EVENTS);
        sorted[..count].copy_from_slice(&events[..count]);
        sort_events(&mut sorted[..count]);

        let mut changed = false;
        for event in &sorted[..count] {
            if event.kind != EventKind::Parameter {
                continue;
            }
            let Some(index) = wire_index(u32::from(event.data1)) else {
                continue;
            };
            if !event.value.is_finite() {
                continue;
            }
            let value = event.value.clamp(0.0, 1.0);
            if self.norm[index] != value {
                self.norm[index] = value;
                changed = true;
            }
        }
        if changed {
            self.apply();
        }
        self.update_host(context);

        audio.clear_outputs();

        if audio.output_channels >= 2 {
            let (left, right) = audio.outputs.split_at_mut(frames);
            let left = &mut left[..frames];
            let right = &mut right[..frames];
            let mut at = 0;
            for event in &sorted[..count] {
                if event.kind == EventKind::Parameter {
                    continue;
                }
                let frame = event.frame.min(frames);
                if frame > at {
                    self.render_stereo(left, right, at, frame);
                    at = frame;
                }
                self.send_midi(event);
            }
            if at < frames {
                self.render_stereo(left, right, at, frames);
            }
        } else {
            let output = audio.output_mut(0);
            let mut at = 0;
            for event in &sorted[..count] {
                if event.kind == EventKind::Parameter {
                    continue;
                }
                let frame = event.frame.min(frames);
                if frame > at {
                    self.render_mono(output, at, frame);
                    at = frame;
                }
                self.send_midi(event);
            }
            if at < frames {
                self.render_mono(output, at, frames);
            }
        }
    }

    fn save_state(&self) -> State {
        let mut bytes = Vec::with_capacity(PARAM_COUNT * 4);
        for value in self.norm {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        State {
            schema_version: 1,
            bytes,
        }
    }

    fn load_state(&mut self, state: &State) -> Result<(), StateError> {
        if state.schema_version > 1 {
            return Err(StateError::SchemaTooNew {
                found: state.schema_version,
                supported: 1,
            });
        }
        if state.bytes.is_empty() {
            return Ok(());
        }
        if state.bytes.len() != PARAM_COUNT * 4 {
            return Err(StateError::Malformed(format!(
                "expected {} bytes",
                PARAM_COUNT * 4
            )));
        }
        for (index, chunk) in state.bytes.chunks_exact(4).enumerate() {
            let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            self.norm[index] = if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                PARAMS[index].normalize(PARAMS[index].default)
            };
        }
        self.apply();
        Ok(())
    }
}

fn descriptor() -> Descriptor {
    DESCRIPTOR
}

fn capabilities() -> Capabilities {
    CAPABILITIES
}

fn create() -> RebitPlugin {
    RebitPlugin::default()
}

export_plugin!(RebitPlugin, descriptor, capabilities, create);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn parameters_are_contiguous_valid_and_uniquely_keyed() {
        let mut keys = HashSet::new();
        for (index, parameter) in PARAMS.iter().enumerate() {
            assert_eq!(parameter.id as usize, index + 1);
            assert!(parameter.is_valid(), "invalid parameter {}", parameter.name);
            assert!(
                keys.insert(key_for(parameter)),
                "duplicate key for {}",
                parameter.name
            );
        }
        assert_eq!(PARAM_COUNT, PARAMS.len());
        assert!(CAPABILITIES.is_valid());
        assert!(DESCRIPTOR.is_valid());
    }

    #[test]
    fn defaults_are_midpoint_normalized_values() {
        let plugin = RebitPlugin::default();
        for (index, parameter) in PARAMS.iter().enumerate() {
            assert!((plugin.norm[index] - parameter.normalize(parameter.default)).abs() < 1e-6);
        }
    }

    #[test]
    fn state_round_trips_all_parameters() {
        let mut first = RebitPlugin::default();
        for (index, value) in first.norm.iter_mut().enumerate() {
            *value = (index as f32 * 0.0137).clamp(0.0, 1.0);
        }
        let state = first.save_state();
        assert_eq!(state.bytes.len(), PARAM_COUNT * 4);

        let mut second = RebitPlugin::default();
        second.load_state(&state).unwrap();
        assert_eq!(first.norm, second.norm);

        second.norm.fill(0.5);
        second.load_state(&State::empty()).unwrap();
        assert_eq!(second.norm[0], 0.5);
        assert!(matches!(
            second.load_state(&State {
                schema_version: 2,
                bytes: Vec::new()
            }),
            Err(StateError::SchemaTooNew { .. })
        ));
    }

    #[test]
    fn note_on_renders_finite_stereo_audio() {
        let mut plugin = RebitPlugin::default();
        let context = ProcessContext {
            block_size: 512,
            ..ProcessContext::default()
        };
        plugin.prepare(&context);

        let frames = context.block_size;
        let mut inputs = Vec::new();
        let mut outputs = vec![0.0f32; 2 * frames];
        let mut sidechain = Vec::new();
        let mut audio = AudioBuffer::new(
            &mut inputs,
            0,
            &mut outputs,
            2,
            &mut sidechain,
            0,
            frames,
        );
        plugin.process(&context, &mut audio, &[Event::note_on(0, 60, 100)]);

        assert!(outputs.iter().all(|value| value.is_finite()));
        assert!(outputs.iter().any(|value| value.abs() > 1e-6));
    }

    #[test]
    fn layered_note_on_fills_both_channels() {
        let mut plugin = RebitPlugin::default();
        plugin.norm[Id::LayerBEnabled.index()] = 1.0;
        plugin.apply();
        let context = ProcessContext {
            block_size: 256,
            ..ProcessContext::default()
        };
        plugin.prepare(&context);

        let mut inputs = Vec::new();
        let mut outputs = vec![0.0f32; 2 * context.block_size];
        let mut sidechain = Vec::new();
        let mut audio = AudioBuffer::new(
            &mut inputs,
            0,
            &mut outputs,
            2,
            &mut sidechain,
            0,
            context.block_size,
        );
        plugin.process(&context, &mut audio, &[Event::note_on(0, 48, 120)]);
        let (left, right) = outputs.split_at(context.block_size);
        assert!(left.iter().any(|value| value.abs() > 1e-6));
        assert!(right.iter().any(|value| value.abs() > 1e-6));
    }

    #[test]
    fn parameter_events_update_the_engine() {
        let mut plugin = RebitPlugin::default();
        let context = ProcessContext::default();
        plugin.prepare(&context);

        let frames = context.block_size;
        let mut inputs = Vec::new();
        let mut outputs = vec![0.0f32; 2 * frames];
        let mut sidechain = Vec::new();
        let mut audio =
            AudioBuffer::new(&mut inputs, 0, &mut outputs, 2, &mut sidechain, 0, frames);

        let normalized = PARAMS[Id::LayerBEnabled.index()].normalize(1.0);
        plugin.process(
            &context,
            &mut audio,
            &[
                Event::note_on(0, 60, 100),
                Event::parameter(0, Id::LayerBEnabled.id(), normalized),
            ],
        );
        assert_eq!(plugin.toggle(Id::LayerBEnabled), 1);
    }

    #[test]
    fn empty_and_unsorted_events_are_safe() {
        let mut plugin = RebitPlugin::default();
        let context = ProcessContext::default();
        plugin.prepare(&context);
        let frames = context.block_size;
        let mut inputs = Vec::new();
        let mut outputs = vec![0.0f32; 2 * frames];
        let mut sidechain = Vec::new();
        let mut audio =
            AudioBuffer::new(&mut inputs, 0, &mut outputs, 2, &mut sidechain, 0, frames);
        plugin.process(
            &context,
            &mut audio,
            &[
                Event::note_off(frames + 100, 60),
                Event::note_on(frames * 2, 60, 100),
            ],
        );
        assert!(outputs.iter().all(|value| value.is_finite()));
    }
}
