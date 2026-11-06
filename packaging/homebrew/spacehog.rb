# Formula for a personal tap (Rohithgilla12/homebrew-tap). Before publishing:
# tag a release, then set `url` to its source tarball and `sha256` to
# `shasum -a 256` of that tarball.
class Spacehog < Formula
  desc "Disk usage TUI and MCP server that knows which bytes are safe to delete"
  homepage "https://github.com/Rohithgilla12/spacehog"
  url "https://github.com/Rohithgilla12/spacehog/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "REPLACE_WITH_RELEASE_TARBALL_SHA256"
  license "MIT"
  head "https://github.com/Rohithgilla12/spacehog.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/cli")
  end

  test do
    (testpath/"proj/node_modules/x").mkpath
    (testpath/"proj/package.json").write "{}"
    (testpath/"proj/node_modules/x/index.js").write "x" * 4096
    ENV["SPACEHOG_DATA_DIR"] = (testpath/"data").to_s
    system bin/"spacehog", "scan", testpath/"proj", "--json"
    assert_match "node_modules", shell_output("#{bin}/spacehog top --json")
  end
end
