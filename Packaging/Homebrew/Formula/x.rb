# Homebrew formula for x.
#
# Place this file in a tap repository (e.g. `homebrew-tap/Formula/x.rb`), then:
#   brew tap xsys/tap
#   brew install x            # builds the stable release
#   brew install --head x     # builds from the main branch
#
# When cutting a tagged release, uncomment the `url`/`sha256` lines and fill in
# the real sha256:
#   curl -fsSL https://github.com/xsys/x/archive/refs/tags/v0.1.0.tar.gz | sha256sum
class X < Formula
  desc "Unified process/port/network/disk/system inspector across platforms"
  homepage "https://github.com/xsys/x"
  license "MIT"
  depends_on "rust" => :build

  # Stable release (fill sha256 after cutting a release):
  # url "https://github.com/xsys/x/archive/refs/tags/v0.1.0.tar.gz"
  # sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  head do
    url "https://github.com/xsys/x.git", branch: "main"
  end

  def install
    system "cargo", "build", "--release", "--locked", "-p", "x-app"
    bin.install "target/release/x"
  end

  test do
    assert_match "x", shell_output("#{bin}/x --version")
  end
end
