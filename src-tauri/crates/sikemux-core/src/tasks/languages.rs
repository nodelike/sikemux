//! What each language's page sets, all through the environment variables its
//! own tools read. The window draws the pages from this list and the brief
//! agents read describes the same settings, so a setting is added in one place.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// The installed JDK to use, kept as `JAVA_HOME`.
    Jdk,
    Text,
    /// On sets the variable to `1`.
    Toggle,
    /// One of `options`, or not set.
    Choice,
    /// Shows the version the folder's files ask for; sets nothing.
    Version,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum When {
    /// The folder builds with Maven.
    Maven,
    /// The build uses Spring Boot.
    SpringBoot,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub section: &'static str,
    pub label: &'static str,
    pub desc: &'static str,
    pub kind: Kind,
    /// Empty for a `Version` field.
    pub variable: &'static str,
    #[serde(skip_serializing_if = "str::is_empty")]
    pub placeholder: &'static str,
    /// Written before what the person types and taken off when shown, such as `-P`.
    #[serde(skip_serializing_if = "str::is_empty")]
    pub prefix: &'static str,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub options: &'static [&'static str],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<When>,
}

const fn field(
    section: &'static str,
    label: &'static str,
    desc: &'static str,
    kind: Kind,
    variable: &'static str,
) -> Field {
    Field {
        section,
        label,
        desc,
        kind,
        variable,
        placeholder: "",
        prefix: "",
        options: &[],
        when: None,
    }
}

const fn text(
    section: &'static str,
    label: &'static str,
    desc: &'static str,
    variable: &'static str,
    placeholder: &'static str,
) -> Field {
    Field {
        placeholder,
        ..field(section, label, desc, Kind::Text, variable)
    }
}

const JAVA: &[Field] = &[
    field(
        "Runtime",
        "JDK",
        "The JDK tasks, terminals and agents here use, as JAVA_HOME.",
        Kind::Jdk,
        "JAVA_HOME",
    ),
    Field {
        prefix: "-P",
        when: Some(When::Maven),
        ..text(
            "Build and run",
            "Maven profiles",
            "Every Maven run here gets them as -P, through MAVEN_ARGS (Maven 3.9 or newer).",
            "MAVEN_ARGS",
            "dev,local",
        )
    },
    Field {
        when: Some(When::SpringBoot),
        ..text(
            "Build and run",
            "Spring profiles",
            "The profiles the app starts with, as SPRING_PROFILES_ACTIVE.",
            "SPRING_PROFILES_ACTIVE",
            "local",
        )
    },
    text(
        "Build and run",
        "JVM options",
        "Given to every java started here, Maven and Gradle included, as JDK_JAVA_OPTIONS.",
        "JDK_JAVA_OPTIONS",
        "-Xmx2g",
    ),
];

const NODE: &[Field] = &[
    field(
        "Runtime",
        "Node",
        "The version the project's .nvmrc or .node-version asks for.",
        Kind::Version,
        "",
    ),
    Field {
        options: &["development", "test", "production"],
        ..field(
            "Runtime",
            "NODE_ENV",
            "Which mode tools and frameworks run in.",
            Kind::Choice,
            "NODE_ENV",
        )
    },
    text(
        "Runtime",
        "Node options",
        "Given to every node started here, as NODE_OPTIONS.",
        "NODE_OPTIONS",
        "--max-old-space-size=4096",
    ),
];

const GO: &[Field] = &[
    field(
        "Build and test",
        "Go",
        "The version go.mod asks for.",
        Kind::Version,
        "",
    ),
    text(
        "Build and test",
        "Go flags",
        "Added to every go build, test and run here, as GOFLAGS.",
        "GOFLAGS",
        "-tags=integration",
    ),
    text(
        "Build and test",
        "Private modules",
        "Fetched straight from their source rather than the public proxy, as GOPRIVATE.",
        "GOPRIVATE",
        "github.com/acme/*",
    ),
];

const RUST: &[Field] = &[
    text(
        "Toolchain and run",
        "Toolchain",
        "The rustup toolchain cargo uses here, as RUSTUP_TOOLCHAIN.",
        "RUSTUP_TOOLCHAIN",
        "stable",
    ),
    text(
        "Toolchain and run",
        "Log filter",
        "What the program logs, as RUST_LOG.",
        "RUST_LOG",
        "info",
    ),
    field(
        "Toolchain and run",
        "Backtraces",
        "Print where a panic happened, as RUST_BACKTRACE=1.",
        Kind::Toggle,
        "RUST_BACKTRACE",
    ),
];

const PYTHON: &[Field] = &[
    field(
        "Runtime",
        "Python",
        "The version the project's .python-version asks for.",
        Kind::Version,
        "",
    ),
    text(
        "Runtime",
        "Import folders",
        "Extra folders Python imports from, as PYTHONPATH.",
        "PYTHONPATH",
        "src",
    ),
    field(
        "Runtime",
        "Unbuffered output",
        "Print output as it happens, as PYTHONUNBUFFERED=1.",
        Kind::Toggle,
        "PYTHONUNBUFFERED",
    ),
];

pub fn fields(language: &str) -> &'static [Field] {
    match language {
        "java" => JAVA,
        "node" => NODE,
        "go" => GO,
        "rust" => RUST,
        "python" => PYTHON,
        _ => &[],
    }
}

/// Every language's fields, for the window to draw the pages from.
pub fn catalog() -> Vec<(&'static str, &'static [Field])> {
    ["java", "node", "go", "rust", "python"]
        .into_iter()
        .map(|language| (language, fields(language)))
        .collect()
}
