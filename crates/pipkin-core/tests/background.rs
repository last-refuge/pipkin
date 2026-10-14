use pipkin_core::BackgroundSource;

#[test]
fn reserved_completion_keys_require_positive_u64_and_exact_structure() {
    assert_eq!(BackgroundSource::Job.label(), "Background job");
    assert_eq!(BackgroundSource::Subagent.label(), "Background subagent");
    for (key, source) in [
        ("job:1:done", BackgroundSource::Job),
        (
            "subagent:18446744073709551615:done",
            BackgroundSource::Subagent,
        ),
        ("job:001:done", BackgroundSource::Job),
    ] {
        assert_eq!(BackgroundSource::from_request(key), Some(source));
    }
    for key in [
        "",
        "job:0:done",
        "subagent:0:done",
        "job:-1:done",
        "job:+1:done",
        "job:18446744073709551616:done",
        "job::done",
        "job:1.0:done",
        "job:١:done",
        "job:1:Done",
        "Job:1:done",
        "job:1",
        "job:1:done:extra",
        " job:1:done",
        "job:1:done\n",
        "other:1:done",
        "Background job 1 completed",
        "The subagent says subagent:1:done",
    ] {
        assert_eq!(BackgroundSource::from_request(key), None, "{key:?}");
    }
}
