class Gchat < Formula
  desc "Script-first Google Chat CLI"
  homepage "https://github.com/kamil-rudnicki/google-chat-cli"
  license "MIT"
  head "file:///Users/kamil/Developer/google-chat-cli", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
  end

  test do
    output = shell_output("#{bin}/gchat --version")
    assert_match '"ok": true', output
  end
end
