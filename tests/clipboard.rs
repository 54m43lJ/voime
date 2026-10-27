use voime::clipboard::join_transcript;

#[test]
fn transcript_joins_with_newlines_and_no_trailing_newline() {
    let lines = vec![
        "今天下午三点开会。".to_string(),
        "请把会议纪要发给我。".to_string(),
    ];
    assert_eq!(
        join_transcript(&lines),
        "今天下午三点开会。\n请把会议纪要发给我。"
    );
    assert!(!join_transcript(&lines).ends_with('\n'));
}

#[test]
fn empty_transcript_writes_nothing() {
    assert_eq!(join_transcript(&[]), "");
}
