use rodio::{Decoder, OutputStream, Sink, Source};
use std::{
    error::Error,
    f32::consts::PI,
    fs::File,
    io::BufReader,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Instant, Duration},
};

struct AudioPlayer {
    sink: Arc<Mutex<Sink>>,
    duration: Duration,
    progress: Arc<Mutex<Duration>>,
    eq_enabled: Arc<AtomicBool>,
    is_playing: Arc<AtomicBool>,
    last_update: Arc<Mutex<Instant>>,
}

impl AudioPlayer {
    fn get_playback_position(&self) -> Duration {
        let mut progress = self.progress.lock().unwrap();
        let mut last_update = self.last_update.lock().unwrap();
        
        if self.is_playing.load(Ordering::Relaxed) {
            let now = Instant::now();
            let elapsed = now.duration_since(*last_update);
            *progress += elapsed;
            *last_update = now;
        }
        
        *progress
    }

    fn play(&self) {
        self.sink.lock().unwrap().play();
        self.is_playing.store(true, Ordering::Relaxed);
        *self.last_update.lock().unwrap() = Instant::now();
    }

    fn pause(&self) {
        self.sink.lock().unwrap().pause();
        self.is_playing.store(false, Ordering::Relaxed);
        self.get_playback_position(); 
    }
}

struct Equalizer<S>
where
    S: Source<Item = f32>,
{
    source: S,
    filters: Vec<BiquadFilter>,
}

struct BiquadFilter {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl BiquadFilter {
    fn new(frequency: f32, q: f32, gain: f32, sample_rate: u32) -> Self {
        let omega = 2.0 * PI * frequency / sample_rate as f32;
        let alpha = omega.sin() / (2.0 * q);
        let a = 10.0f32.powf(gain / 40.0);

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * omega.cos();
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * omega.cos();
        let a2 = 1.0 - alpha / a;

        BiquadFilter {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.b0 * input + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;
        output
    }
}

impl<S> Equalizer<S>
where
    S: Source<Item = f32>,
{
    fn new(source: S, gains: Vec<f32>) -> Self {
        let sample_rate = source.sample_rate();
        let frequencies = [
            32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
        ];
        let filters = frequencies
            .iter()
            .zip(gains.iter())
            .map(|(&freq, &gain)| BiquadFilter::new(freq, 1.41, gain, sample_rate))
            .collect();

        Equalizer { source, filters }
    }
}

impl<S> Iterator for Equalizer<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        self.source.next().map(|sample| {
            self.filters
                .iter_mut()
                .fold(sample, |s, filter| filter.process(s))
        })
    }
}

impl<S> Source for Equalizer<S>
where
    S: Source<Item = f32>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.source.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.source.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.source.total_duration()
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let (_stream, stream_handle) =
        OutputStream::try_default().expect("Failed to get default output device");
    let sink = Sink::try_new(&stream_handle).expect("Failed to create sink");
    let sink = Arc::new(Mutex::new(sink));

    let file = BufReader::new(File::open("outaspace.flac").expect("Failed to open file"));
    let decoder = Decoder::new(file)
        .expect("Failed to create decoder")
        .convert_samples::<f32>();
    let duration = decoder.total_duration().unwrap_or(Duration::from_secs(0));

    let db_gains = vec![4.6, 8.0, 4.6, 0.9, 0.0, 3.0, 0.9, 0.0, 0.0, 0.0];
    let equalizer = Equalizer::new(decoder, db_gains);
    sink.lock().unwrap().append(equalizer);

    let audio_player = AudioPlayer {
        sink,
        duration,
        progress: Arc::new(Mutex::new(Duration::from_secs(0))),
        eq_enabled: Arc::new(AtomicBool::new(true)),
        is_playing: Arc::new(AtomicBool::new(false)),
        last_update: Arc::new(Mutex::new(Instant::now())),
    };

    audio_player.play();

    std::thread::sleep(duration);

    Ok(())
}

fn seek(audio_player: &AudioPlayer, position: Duration, toggle_eq: bool) {
    let sink = audio_player.sink.lock().unwrap();
    let was_playing = audio_player.is_playing.load(Ordering::Relaxed);

    sink.stop();
    audio_player.is_playing.store(false, Ordering::Relaxed);

    let file = BufReader::new(File::open("outaspace.flac").expect("Failed to open file"));
    let decoder = Decoder::new(file)
        .expect("Failed to create decoder")
        .convert_samples::<f32>();

    let db_gains = vec![6.0, 3.0, 0.0, 0.0, -3.0, -6.0, 0.0, 3.0, 6.0, 3.0];

    if toggle_eq {
        let current_state = audio_player.eq_enabled.load(Ordering::Relaxed);
        audio_player.eq_enabled.store(!current_state, Ordering::Relaxed);
    }

    let source = if audio_player.eq_enabled.load(Ordering::Relaxed) {
        Box::new(Equalizer::new(decoder, db_gains)) as Box<dyn Source<Item = f32> + Send>
    } else {
        Box::new(decoder) as Box<dyn Source<Item = f32> + Send>
    };

    sink.append(source.skip_duration(position));
    *audio_player.progress.lock().unwrap() = position;
    *audio_player.last_update.lock().unwrap() = Instant::now();

    if was_playing {
        sink.play();
        audio_player.is_playing.store(true, Ordering::Relaxed);
    }
}
