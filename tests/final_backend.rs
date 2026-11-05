use voime_gpu_common::{
    expect_result, read_frame, write_frame, ErrorCode, Frame, GpuBackend, Request, Response,
    GPU_PROTOCOL_VERSION,
};
use voimed::backend::BackendId;

#[test]
fn backend_selection_lists_only_its_models() {
    assert!(BackendId::Cpu.model_dirs().is_empty());
    assert_eq!(BackendId::Qwen3.model_dirs(), &["qwen3-asr-1.7b"]);
    assert!(BackendId::Cpu.worker_binary().is_none());
    assert!(BackendId::Qwen3.worker_binary().is_some());
}

#[test]
fn gpu_result_is_matched_by_id_and_text_is_preserved() {
    let response = Response::Result {
        id: 9,
        text: "最终文本。".to_string(),
        latency_ms: 10,
        rtf: 0.1,
    };
    let matched = expect_result(response, 9).unwrap();
    assert!(matches!(matched, Response::Result { .. }));
    let error = Response::Error {
        id: 3,
        code: ErrorCode::InferenceFailed,
        message: "failed".to_string(),
    };
    assert!(expect_result(error, 9).is_err());
}

#[test]
fn wire_frame_round_trips_transcribe() {
    let request = Request::Transcribe {
        id: 1,
        sample_rate: 16_000,
        language: "zh".to_string(),
    };
    let frame = Frame::from_request(&request, &[0.0, 0.25, -0.25]).unwrap();
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &frame).unwrap();
    let mut cursor = std::io::Cursor::new(bytes);
    let decoded = read_frame(&mut cursor).unwrap().unwrap();
    assert_eq!(decoded.audio, vec![0.0, 0.25, -0.25]);
    assert_eq!(decoded.request().unwrap(), request);
}

#[test]
fn gpu_protocol_is_version_one() {
    assert_eq!(GPU_PROTOCOL_VERSION, 1);
    assert_eq!(GpuBackend::Qwen3.as_str(), "qwen3");
}
