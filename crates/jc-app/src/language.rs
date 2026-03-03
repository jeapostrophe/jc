use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Language {
  Rust,
  JavaScript,
  TypeScript,
  Tsx,
  Python,
  Ruby,
  Go,
  Markdown,
  Toml,
  Json,
  Yaml,
  Html,
  Css,
  C,
  Cpp,
  Java,
  Bash,
  #[default]
  Text,
}

impl Language {
  pub fn name(self) -> &'static str {
    match self {
      Self::Rust => "rust",
      Self::JavaScript => "javascript",
      Self::TypeScript => "typescript",
      Self::Tsx => "tsx",
      Self::Python => "python",
      Self::Ruby => "ruby",
      Self::Go => "go",
      Self::Markdown => "markdown",
      Self::Toml => "toml",
      Self::Json => "json",
      Self::Yaml => "yaml",
      Self::Html => "html",
      Self::Css => "css",
      Self::C => "c",
      Self::Cpp => "cpp",
      Self::Java => "java",
      Self::Bash => "bash",
      Self::Text => "text",
    }
  }

  pub fn from_extension(ext: &str) -> Self {
    match ext {
      "rs" => Self::Rust,
      "js" => Self::JavaScript,
      "ts" => Self::TypeScript,
      "tsx" => Self::Tsx,
      "py" => Self::Python,
      "rb" => Self::Ruby,
      "go" => Self::Go,
      "c" | "h" => Self::C,
      "cpp" | "cc" | "cxx" | "hpp" => Self::Cpp,
      "java" => Self::Java,
      "md" => Self::Markdown,
      "toml" => Self::Toml,
      "json" => Self::Json,
      "yaml" | "yml" => Self::Yaml,
      "html" => Self::Html,
      "css" => Self::Css,
      "sh" | "bash" | "zsh" => Self::Bash,
      _ => Self::Text,
    }
  }

  pub fn from_path(path: &Path) -> Self {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    Self::from_extension(ext)
  }
}
