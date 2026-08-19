//! Integration test: compact a checked-in CDP `Accessibility.getFullAXTree` dump.

use xai_grok_browser::host::{SNAPSHOT_NODE_CAP, compact_ax_tree};

#[test]
fn compact_ax_example_fixture() {
    let json = include_str!("fixtures/ax_example.json");
    let nodes = compact_ax_tree(json, false).expect("compact fixture");
    assert!(nodes.len() <= SNAPSHOT_NODE_CAP);
    assert_eq!(nodes.len(), 3);
    // Heading sits under the ignored generic wrapper (`childIds: ["2"]`).
    // Uids carry the `ax-` prefix: they are numbered over the AX tree, not the
    // tagged DOM, so they must never resolve against `data-turbo-uid`.
    assert_eq!(nodes[0].uid, "ax-1");
    assert_eq!(nodes[0].role, "heading");
    assert_eq!(nodes[0].name, "Example Domain");
    assert!(
        nodes.iter().any(|n| n.uid == "ax-1" && n.role == "heading"),
        "descendant of ignored wrapper must still get a uid"
    );
    assert_eq!(nodes[1].uid, "ax-2");
    assert_eq!(nodes[1].role, "link");
    assert_eq!(nodes[1].name, "More information...");
    assert_eq!(nodes[2].uid, "ax-3");
    assert_eq!(nodes[2].role, "textbox");
    assert_eq!(nodes[2].name, "Search");
    assert_eq!(nodes[2].value.as_deref(), Some("query"));
    assert!(nodes[2].focused);
    assert!(nodes.iter().all(|n| n.role != "RootWebArea"));
}
