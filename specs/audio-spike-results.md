# Windows audio spike findings

## Commands

In Developer PowerShell from the repository root:

```powershell
cargo run --example audio_probe -- list
cargo run --example audio_probe -- run '<CABLE Output capture ID>' '<headphones playback ID>'
cargo run --example audio_probe -- run missing '<headphones playback ID>'
```

See the root README for routing and feedback precautions. The `run` probe lasts ten seconds, then joins both audio threads. On this WSL-hosted checkout, Windows Cargo required the Visual Studio SDK library paths in `LIB` and a separate Windows `CARGO_TARGET_DIR`; a Developer PowerShell supplies these paths automatically.

## Observed on host

- Windows 10 Pro 10.0.19045, x64; Windows Rust 1.99.0; `wasapi` 0.26.0 (MIT, target-gated; built on the Windows host). Windows MSVC/SDK installed.
- `list` successfully enumerated active capture and render endpoints by opaque WASAPI ID. `CABLE Output (VB-Audio Virtual Cable)` is capture: `{0.0.1.00000000}.{7c016758-fdb5-46bc-9715-10e2cf1fa78b}`. `CABLE Input` is playback: `{0.0.0.00000000}.{dbf66f53-d54f-4171-9624-39083fb6a260}`. Selected render device for the first run was `Lautsprecher (2- Realtek High Definition Audio(SST))`: `{0.0.0.00000000}.{9b223774-3716-45cd-b861-eb5adb91dd34}`. Also detected `Kopfhörer (2- MOMENTUM 4 Stereo)`, `{0.0.0.00000000}.{fbb55114-4858-4f51-b13b-9029c1146fb8}`.
- CABLE Output: shared mix format 48 kHz stereo f32; device format 48 kHz stereo 24-bit PCM; default/min periods 10/3 ms; 48 kHz stereo f32 reported supported in shared mode. Realtek playback: shared mix 48 kHz stereo f32, device format 48 kHz stereo 16-bit PCM, default/min 10/2 ms; requested float format reported supported. MOMENTUM 4 Stereo playback: mix 44.1 kHz stereo f32, device 44.1 kHz stereo 16-bit PCM; default/min 10/3 ms; `is_supported` returned 44.1 kHz closest to 48 kHz. The listing also includes Voicemeeter endpoints and a 16 kHz mono hands-free headset; `list` prints each endpoint's mix/device formats, periods, and 48 kHz float query result (not an exhaustive capability scan).
- The first trial found that `get_next_packet_size()` may return `Some(0)` when no capture frames are available. The initial loop spun and failed to stop; it was terminated and the loop corrected to break on zero. A subsequent ten-second capture-to-Realtek shared-mode render run exited normally: capture buffer 1056 frames, render buffer 1056 frames, captured 470880 frames, rendered 394080 frames, dropped 76320 queued capture frames, peak 0.000 (no application routed to CABLE Input during this trial). One initial capture discontinuity was reported at frame zero. Audio audibility was **not** established by this silent run.
- An invalid capture ID (`missing`) returned an actionable endpoint error and exit code 1 without starting either worker.
- A second ten-second silent run opened CABLE Output capture and MOMENTUM 4 Stereo render in 48 kHz stereo float shared mode even though the latter's mix format is 44.1 kHz; capture 473760 frames, render 436354 frames, peak 0.000, normal exit. Shared-mode automatic conversion accepted the requested rate; audible conversion quality is still unverified.
- With the user playing a browser video routed to CABLE Input and Spotify routed to Realtek, the ten-second CABLE Output → Realtek run exited normally: captured 473760 frames, peak 0.623, dropped 0 queued frames, rendered 476160 frames. The user confirmed that the browser video and Spotify were both audible through the Realtek speakers at the same time.
- After review fixes, a registered but unplugged Jabra capture ID was rejected before starting workers with an `Unplugged` error and a reconnect/relist hint (exit 1). CABLE Output capture paired with CABLE Input playback was refused before starting workers with a feedback warning (exit 1). A valid CABLE Output → Realtek run still exited normally after ten seconds (peak 0.772); this later run had 93120 queued capture frames dropped and was not independently listened to.
- After the follow-up feedback fix, the probe rejects capture and playback on the same audio adapter (rather than matching changeable endpoint names). The CABLE Output → CABLE Input pair returned the feedback warning and exit 1; CABLE Output → Realtek still exited normally after ten seconds (peak 0.000 on this silent trial). An actual endpoint rename was not performed on the host.
- With the adapter-based guard in place, the user repeated the browser-to-CABLE Input and Spotify-to-Realtek setup. The CABLE Output → Realtek run exited normally after ten seconds: captured 477120 frames, peak 0.467, dropped 0 queued frames, rendered 478560 frames. The user confirmed both browser video and Spotify were audible through Realtek simultaneously.

## Still to verify on hardware

- Repeat on the 44.1 kHz playback endpoint (MOMENTUM 4 Stereo) with actual sound to assess conversion quality. Test unplug/disconnect and stop behavior during streaming; the timed run and missing endpoint test alone do not establish these cases.
- Determine whether software-adjustable playback resampling is practical with this path; `wasapi` offers shared-mode format conversion but no adjustable playback-ratio control in the APIs inspected. Task 02 should keep a separate clock-correction/resampler decision open.

## Task 02 interface recommendation (provisional)

`wasapi` 0.26.0 is recommended for the first Windows backend based on the audible shared-mode CABLE Output → Realtek test. Expose opaque endpoint IDs, names, and capture/playback kind across the audio boundary; initialize COM and create/use WASAPI clients on their owning worker thread. Keep device-native conversion separate from adjustable playback resampling, and use bounded queues between capture and playback with an explicit overflow policy. Disconnect and audible 44.1 kHz conversion checks remain open.
