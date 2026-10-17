//! Which language a file is in, how to colour it, and which language server helps with it.

use std::path::Path;

/// A language the editor knows about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Language {
    /// The name the highlighter knows it by.
    pub grammar: &'static str,
    /// The name shown to the user.
    pub label: &'static str,
    /// The LSP language id sent with each document.
    pub lsp_id: &'static str,
    /// Language servers to try, in order: the program, then its arguments.
    pub servers: &'static [&'static [&'static str]],
    /// Files that mark the top folder of a project in this language.
    pub roots: &'static [&'static str],
}

const RUST: Language = Language {
    grammar: "rust",
    label: "Rust",
    lsp_id: "rust",
    servers: &[&["rust-analyzer"]],
    roots: &["Cargo.toml"],
};
const TYPESCRIPT_SERVERS: &[&[&str]] = &[&["typescript-language-server", "--stdio"]];
const JS_ROOTS: &[&str] = &["package.json", "tsconfig.json", "jsconfig.json"];
const PYTHON_SERVERS: &[&[&str]] = &[
    &["basedpyright-langserver", "--stdio"],
    &["pyright-langserver", "--stdio"],
    &["pylsp"],
];
const C_SERVERS: &[&[&str]] = &[&["clangd"]];
const C_ROOTS: &[&str] = &["compile_commands.json", "CMakeLists.txt", "Makefile"];

/// Finds the language from the file name, or None for plain text.
pub fn detect(path: &Path) -> Option<Language> {
    let name = path.file_name()?.to_str()?;
    let lower = name.to_ascii_lowercase();
    let by_name = match lower.as_str() {
        "makefile" | "gnumakefile" => Some(lang("make", "Makefile", "makefile", &[], &[])),
        "cmakelists.txt" => Some(lang("cmake", "CMake", "cmake", &[], &[])),
        ".bashrc" | ".bash_profile" | ".zshrc" | ".zprofile" | ".zshenv" | ".profile" => Some(shell()),
        "cargo.lock" => Some(toml()),
        _ => None,
    };
    if by_name.is_some() {
        return by_name;
    }
    let ext = lower.rsplit_once('.').map(|(_, ext)| ext)?;
    Some(match ext {
        "rs" => RUST,
        "toml" => toml(),
        "json" | "jsonc" => lang("json", "JSON", "json", &[&["vscode-json-language-server", "--stdio"]], &[]),
        "js" | "mjs" | "cjs" | "jsx" => lang("javascript", "JavaScript", "javascript", TYPESCRIPT_SERVERS, JS_ROOTS),
        "ts" | "mts" | "cts" => lang("typescript", "TypeScript", "typescript", TYPESCRIPT_SERVERS, JS_ROOTS),
        "tsx" => lang("tsx", "TSX", "typescriptreact", TYPESCRIPT_SERVERS, JS_ROOTS),
        "py" | "pyi" => lang("python", "Python", "python", PYTHON_SERVERS, &["pyproject.toml", "setup.py"]),
        "go" => lang("go", "Go", "go", &[&["gopls"]], &["go.mod"]),
        "sh" | "bash" | "zsh" => shell(),
        "yml" | "yaml" => lang("yaml", "YAML", "yaml", &[&["yaml-language-server", "--stdio"]], &[]),
        "md" | "markdown" | "mdx" => lang("markdown", "Markdown", "markdown", &[&["marksman", "server"]], &[]),
        "html" | "htm" => lang("html", "HTML", "html", &[&["vscode-html-language-server", "--stdio"]], &[]),
        "css" | "scss" => lang("css", "CSS", "css", &[&["vscode-css-language-server", "--stdio"]], &[]),
        "c" | "h" => lang("c", "C", "c", C_SERVERS, C_ROOTS),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => lang("cpp", "C++", "cpp", C_SERVERS, C_ROOTS),
        "cs" => lang("csharp", "C#", "csharp", &[], &[]),
        "java" => lang("java", "Java", "java", &[&["jdtls"]], &["pom.xml", "build.gradle"]),
        "kt" | "kts" => lang("kotlin", "Kotlin", "kotlin", &[&["kotlin-language-server"]], &[]),
        "lua" => lang("lua", "Lua", "lua", &[&["lua-language-server"]], &[]),
        "rb" => lang("ruby", "Ruby", "ruby", &[&["ruby-lsp"]], &["Gemfile"]),
        "php" => lang("php", "PHP", "php", &[&["intelephense", "--stdio"]], &["composer.json"]),
        "swift" => lang("swift", "Swift", "swift", &[&["sourcekit-lsp"]], &["Package.swift"]),
        "zig" => lang("zig", "Zig", "zig", &[&["zls"]], &["build.zig"]),
        "sql" => lang("sql", "SQL", "sql", &[], &[]),
        "proto" => lang("proto", "Protocol Buffers", "proto", &[], &[]),
        "graphql" | "gql" => lang("graphql", "GraphQL", "graphql", &[], &[]),
        "ex" | "exs" => lang("elixir", "Elixir", "elixir", &[&["elixir-ls"]], &["mix.exs"]),
        "scala" => lang("scala", "Scala", "scala", &[&["metals"]], &["build.sbt"]),
        "svelte" => lang("svelte", "Svelte", "svelte", &[&["svelteserver", "--stdio"]], JS_ROOTS),
        "astro" => lang("astro", "Astro", "astro", &[], JS_ROOTS),
        "diff" | "patch" => lang("diff", "Diff", "diff", &[], &[]),
        "mk" => lang("make", "Makefile", "makefile", &[], &[]),
        "cmake" => lang("cmake", "CMake", "cmake", &[], &[]),
        _ => return None,
    })
}

const fn lang(
    grammar: &'static str,
    label: &'static str,
    lsp_id: &'static str,
    servers: &'static [&'static [&'static str]],
    roots: &'static [&'static str],
) -> Language {
    Language {
        grammar,
        label,
        lsp_id,
        servers,
        roots,
    }
}

const fn toml() -> Language {
    lang("toml", "TOML", "toml", &[&["taplo", "lsp", "stdio"]], &[])
}

const fn shell() -> Language {
    lang("bash", "Shell", "shellscript", &[&["bash-language-server", "start"]], &[])
}

/// The project folder a file belongs to: the nearest one under version control, else
/// the nearest with one of `markers`, else the file's own folder.
pub fn project_root(file: &Path, markers: &[&str]) -> std::path::PathBuf {
    let dir = file.parent().unwrap_or(file);
    let git = dir.ancestors().find(|dir| dir.join(".git").exists());
    let marked = || dir.ancestors().find(|dir| markers.iter().any(|m| dir.join(m).exists()));
    git.or_else(marked).unwrap_or(dir).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_languages_by_name_and_extension() {
        assert_eq!(detect(Path::new("/a/main.rs")).map(|l| l.grammar), Some("rust"));
        assert_eq!(detect(Path::new("/a/Cargo.toml")).map(|l| l.grammar), Some("toml"));
        assert_eq!(detect(Path::new("/a/App.TSX")).map(|l| l.lsp_id), Some("typescriptreact"));
        assert_eq!(detect(Path::new("/a/Makefile")).map(|l| l.grammar), Some("make"));
        assert_eq!(detect(Path::new("/a/.zshrc")).map(|l| l.grammar), Some("bash"));
        assert_eq!(detect(Path::new("/a/.env.example")), None);
        assert_eq!(detect(Path::new("/a/notes")), None);
    }

    #[test]
    fn root_is_the_nearest_marked_folder() {
        let base = std::env::temp_dir().join(format!("somme-root-{}", std::process::id()));
        std::fs::create_dir_all(base.join("crate/src")).unwrap();
        std::fs::write(base.join("crate/Cargo.toml"), "").unwrap();
        let root = project_root(&base.join("crate/src/main.rs"), &["Cargo.toml"]);
        let plain = project_root(&base.join("crate/src/main.rs"), &["go.mod"]);
        std::fs::remove_dir_all(&base).unwrap();
        assert_eq!(root, base.join("crate"));
        // No marker and no git folder (the temp folder is outside any repo).
        assert_eq!(plain, base.join("crate/src"));
    }
}
