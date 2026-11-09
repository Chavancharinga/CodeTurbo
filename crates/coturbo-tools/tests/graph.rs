use coturbo_tools::graph;

#[test]
fn the_graph_is_pointed_out_only_when_it_exists() {
    let dir = tempfile::tempdir().unwrap();
    assert!(graph::note(dir.path()).is_empty());
    std::fs::create_dir_all(dir.path().join(graph::OUT)).unwrap();
    std::fs::write(dir.path().join(graph::OUT).join("GRAPH_REPORT.md"), "# Graph").unwrap();
    let note = graph::note(dir.path());
    assert!(note.contains("GRAPH_REPORT.md") && note.contains("graphify query") && note.contains("graphify affected"), "{note}");
}

#[test]
fn a_folder_outside_git_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    graph::refresh(dir.path());
    assert!(!dir.path().join(graph::OUT).exists());
}
