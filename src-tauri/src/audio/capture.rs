use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::broadcast;

pub struct AudioCapture {
    // This will hold the streams so they don't get dropped and stop recording
    _input_stream: Option<cpal::Stream>,
    _output_stream: Option<cpal::Stream>,
}

impl AudioCapture {
    pub fn new() -> Self {
        Self {
            _input_stream: None,
            _output_stream: None,
        }
    }

    pub fn start(
        &mut self,
        mic_tx: broadcast::Sender<Vec<f32>>,
        sys_tx: broadcast::Sender<Vec<f32>>,
    ) -> Result<((u32, u16), (u32, u16)), String> {
        let host = cpal::default_host();

        println!("--- Audio Device Discovery ---");
        if let Ok(devices) = host.output_devices() {
            for (idx, device) in devices.enumerate() {
                let name = device.name().unwrap_or_else(|_| "Unknown".to_string());
                if let Ok(config) = device.default_output_config() {
                    println!(
                        "Device [{}]: {} (Channels: {}, Rate: {}Hz)",
                        idx,
                        name,
                        config.channels(),
                        config.sample_rate()
                    );
                }
            }
        }
        println!("------------------------------");

        // Setup microphone capture (Input)
        let input_device = host
            .default_input_device()
            .ok_or("Failed to get default input device")?;
        println!(
            "Using input device: {}",
            input_device
                .name()
                .unwrap_or_else(|_| "Unknown".to_string())
        );

        let input_config = input_device
            .default_input_config()
            .map_err(|e| e.to_string())?;
        println!("Input config: {:?}", input_config);

        let mic_sample_rate = input_config.sample_rate();
        let mic_channels = input_config.channels();

        let tx_mic = mic_tx.clone();
        let mut mic_count = 0;
        let input_stream = input_device
            .build_input_stream(
                &input_config.clone().into(),
                move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                    mic_count += 1;
                    if mic_count % 100 == 0 {
                        println!("Mic callback triggered: samples={}", data.len());
                    }
                    if mic_channels == 2 {
                        let mut mono_data = Vec::with_capacity(data.len() / 2);
                        for chunk in data.chunks_exact(2) {
                            mono_data.push((chunk[0] + chunk[1]) / 2.0);
                        }
                        let _ = tx_mic.send(mono_data);
                    } else if mic_channels == 1 {
                        let _ = tx_mic.send(data.to_vec());
                    } else {
                        let mut mono_data = Vec::with_capacity(data.len() / mic_channels as usize);
                        for chunk in data.chunks_exact(mic_channels as usize) {
                            mono_data.push(chunk[0]);
                        }
                        let _ = tx_mic.send(mono_data);
                    }
                },
                |err| eprintln!("an error occurred on input stream: {}", err),
                None,
            )
            .map_err(|e| e.to_string())?;

        // Setup system audio capture (Output loopback)
        let output_device = host
            .default_output_device()
            .ok_or("Failed to get default output device")?;
        println!(
            "Using output device for loopback: {}",
            output_device
                .name()
                .unwrap_or_else(|_| "Unknown".to_string())
        );

        let output_config = output_device
            .default_output_config()
            .map_err(|e| e.to_string())?;
        println!("Output config: {:?}", output_config);

        let sys_sample_rate = output_config.sample_rate();
        let sys_channels = output_config.channels();

        let tx_sys = sys_tx.clone();
        let mut sys_count = 0;
        let output_stream = output_device
            .build_input_stream(
                &output_config.clone().into(),
                move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                    sys_count += 1;

                    let mut mono_data = Vec::with_capacity(data.len() / sys_channels as usize);

                    // 1. Find which channel is the "dominant" one in this chunk
                    let mut channel_peaks = vec![0.0f32; sys_channels as usize];
                    for chunk in data.chunks_exact(sys_channels as usize) {
                        for (i, &sample) in chunk.iter().enumerate() {
                            channel_peaks[i] = channel_peaks[i].max(sample.abs());
                        }
                    }

                    let (best_channel_idx, &max_peak) = channel_peaks
                        .iter()
                        .enumerate()
                        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
                        .unwrap_or((0, &0.0));

                    // 2. Extract only that channel
                    for chunk in data.chunks_exact(sys_channels as usize) {
                        mono_data.push(chunk[best_channel_idx]);
                    }

                    if sys_count % 100 == 0 {
                        println!(
                            "Sys capture | Best Ch: {} | Peak: {:.4}",
                            best_channel_idx, max_peak
                        );
                    }
                    let _ = tx_sys.send(mono_data);
                },
                |err| eprintln!("an error occurred on output loopback stream: {}", err),
                None,
            )
            .map_err(|e| e.to_string())?;

        input_stream.play().map_err(|e| e.to_string())?;
        output_stream.play().map_err(|e| e.to_string())?;

        self._input_stream = Some(input_stream);
        self._output_stream = Some(output_stream);

        if mic_sample_rate != sys_sample_rate {
            eprintln!(
                "WARNING: Sample rate mismatch! Mic: {}Hz, Sys: {}Hz. This may cause sync issues.",
                mic_sample_rate, sys_sample_rate
            );
        }

        println!(
            "Dual audio capture started. Mic: {}Hz, {}ch | Sys: {}Hz, {}ch",
            mic_sample_rate, mic_channels, sys_sample_rate, sys_channels
        );
        Ok((
            (mic_sample_rate, mic_channels),
            (sys_sample_rate, 1), // Stream is explicitly downmixed to Mono
        ))
    }
}
