pub mod audio;
pub mod constants;
pub mod lang_id;
pub mod text;
pub mod utils;
pub mod vad;

pub use audio::{
    AudioRecorder, CpalDeviceInfo, FrameObserver, RecordingStart, VadPolicy,
    is_microphone_access_denied, is_no_input_device_error, list_input_devices, list_output_devices,
    read_wav_samples, save_wav_file, verify_wav_file,
};
pub use lang_id::detect_output_language;
pub use text::{
    Fix, OutputLanguageEvidence, TimedText, apply_custom_words, apply_custom_words_to_timed,
    apply_custom_words_with_fixes, build_match_key, expand_snippets, expand_snippets_with_fixes,
    normalize_transcription_output, remove_filler_words, scrub_before_keyword,
    strip_trailing_keyword,
};
pub use utils::get_cpal_host;
pub use vad::{EarshotVad, SileroVad, VoiceActivityDetector};
