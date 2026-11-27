//! The dependency changes exercised by Test 3 and reused by later checks.

pub struct Case {
    pub id: &'static str,
    pub label: &'static str,
    pub fixture: &'static str,
    /// The dependency change, as arguments to an allowed Cargo subcommand.
    pub change: &'static [&'static str],
    /// Package-id fragments that must be present after the change.
    pub expect_present: &'static [&'static str],
    /// Package-id fragments that must be absent after the change.
    pub expect_absent: &'static [&'static str],
    /// A fragment that must appear in some resolved edge.
    pub expect_edge: (&'static str, &'static str),
    /// Counted among the five cases of the brief, or an extra.
    pub counted: bool,
}

pub const CASES: &[Case] = &[
    Case {
        id: "1",
        label: "add a crate that is not in the tree",
        fixture: "small",
        change: &["add", "getrandom@=0.2.15", "-p", "small-app"],
        expect_present: &["#getrandom@0.2.15", "#wasi@"],
        expect_absent: &[],
        expect_edge: ("app#small-app@0.1.0 -> ", "#getrandom@0.2.15"),
        counted: true,
    },
    Case {
        id: "2",
        label: "upgrade an existing direct dependency",
        fixture: "small",
        change: &["add", "smallvec@=1.6.1", "-p", "small-app"],
        expect_present: &["#smallvec@1.6.1"],
        expect_absent: &["#smallvec@1.6.0"],
        expect_edge: ("app#small-app@0.1.0 -> ", "#smallvec@1.6.1"),
        counted: true,
    },
    Case {
        id: "3",
        label: "add a crate with a non-default feature",
        fixture: "small",
        change: &["add", "serde@=1.0.210", "-p", "small-app", "--features", "derive"],
        expect_present: &["#serde@1.0.210", "#serde_derive@1.0.210", "#syn@"],
        expect_absent: &[],
        expect_edge: ("#serde@1.0.210 -> ", "#serde_derive@1.0.210"),
        counted: true,
    },
    Case {
        id: "4",
        label: "add a crate to one member of the workspace",
        fixture: "small",
        change: &["add", "either@=1.13.0", "-p", "small-core"],
        expect_present: &["#either@1.13.0"],
        expect_absent: &[],
        expect_edge: ("core#small-core@0.1.0 -> ", "#either@1.13.0"),
        counted: true,
    },
    Case {
        id: "5",
        label: "add a crate that is not in the tree",
        fixture: "public",
        change: &["add", "petgraph@=0.6.5"],
        expect_present: &["#petgraph@0.6.5", "#fixedbitset@"],
        expect_absent: &[],
        expect_edge: ("#bat@0.25.0 -> ", "#petgraph@0.6.5"),
        counted: true,
    },
    Case {
        id: "extra",
        label: "change a locked version with cargo update --precise",
        fixture: "small",
        change: &["update", "itoa", "--precise", "1.0.10"],
        expect_present: &["#itoa@1.0.10"],
        expect_absent: &[],
        expect_edge: ("app#small-app@0.1.0 -> ", "#itoa@1.0.10"),
        counted: false,
    },
];
