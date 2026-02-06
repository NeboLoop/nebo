# Homebrew formula for Nebo
# To install: brew install nebolabs/tap/nebo
# Or: brew tap nebolabs/tap && brew install nebo
#
# NOTE: The canonical formula lives in github.com/nebolabs/homebrew-tap
# This copy is for reference only.

class Nebo < Formula
  desc "AI agent with web UI - your personal AI companion"
  homepage "https://github.com/nebolabs/nebo"
  version "0.1.1"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/nebolabs/nebo/releases/download/v#{version}/nebo-darwin-arm64"
      sha256 "253ed36908a0ce00626a45e7f6763ba271a73389c5bfc3a9605260f687a31816"

      def install
        bin.install "nebo-darwin-arm64" => "nebo"
      end
    end

    on_intel do
      url "https://github.com/nebolabs/nebo/releases/download/v#{version}/nebo-darwin-amd64"
      sha256 "604460c2dcc82db1beb5db55e7da248f19b355434622d42caf3db8b491490eed"

      def install
        bin.install "nebo-darwin-amd64" => "nebo"
      end
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/nebolabs/nebo/releases/download/v#{version}/nebo-linux-arm64"
      sha256 "fab8e87da1c38e2b67685f3fc9c64b79b058f56065fe32a4f69f343322339859"

      def install
        bin.install "nebo-linux-arm64" => "nebo"
      end
    end

    on_intel do
      url "https://github.com/nebolabs/nebo/releases/download/v#{version}/nebo-linux-amd64"
      sha256 "74d9d68bca2c917cc747e5962be9e7d836ab508da4d5836f8d9fd8dd4e83d369"

      def install
        bin.install "nebo-linux-amd64" => "nebo"
      end
    end
  end

  def caveats
    <<~EOS
      Nebo installed successfully!

      To start Nebo (desktop mode with native window):
        nebo

      To start headless (browser-only):
        nebo --headless

      First time setup:
        1. Add API keys in Settings > Providers
        2. Start chatting!

      Data is stored in platform-standard locations:
        macOS:  ~/Library/Application Support/Nebo/
        Linux:  ~/.config/nebo/
    EOS
  end

  test do
    assert_match "nebo", shell_output("#{bin}/nebo --help")
  end
end
