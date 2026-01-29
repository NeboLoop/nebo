# Homebrew formula for GoBot
# To install: brew install localrivet/tap/gobot
# Or: brew tap localrivet/tap && brew install gobot

class Gobot < Formula
  desc "AI agent with web UI - your personal AI companion"
  homepage "https://github.com/localrivet/gobot"
  version "0.1.0"  # TODO: Update version
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/localrivet/gobot/releases/download/v#{version}/gobot-darwin-arm64"
      sha256 "PLACEHOLDER_SHA256_ARM64"  # TODO: Update after release

      def install
        bin.install "gobot-darwin-arm64" => "gobot"
      end
    end

    on_intel do
      url "https://github.com/localrivet/gobot/releases/download/v#{version}/gobot-darwin-amd64"
      sha256 "PLACEHOLDER_SHA256_AMD64"  # TODO: Update after release

      def install
        bin.install "gobot-darwin-amd64" => "gobot"
      end
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/localrivet/gobot/releases/download/v#{version}/gobot-linux-arm64"
      sha256 "PLACEHOLDER_SHA256_LINUX_ARM64"  # TODO: Update after release

      def install
        bin.install "gobot-linux-arm64" => "gobot"
      end
    end

    on_intel do
      url "https://github.com/localrivet/gobot/releases/download/v#{version}/gobot-linux-amd64"
      sha256 "PLACEHOLDER_SHA256_LINUX_AMD64"  # TODO: Update after release

      def install
        bin.install "gobot-linux-amd64" => "gobot"
      end
    end
  end

  def caveats
    <<~EOS
      GoBot installed successfully!

      To start GoBot:
        gobot

      Then open http://localhost:27895 in your browser.

      First time setup:
        1. Go to http://localhost:27895/setup
        2. Create your admin account
        3. Add API keys in Settings > Providers

      Configuration is stored in ~/.gobot/
    EOS
  end

  test do
    assert_match "GoBot", shell_output("#{bin}/gobot --version")
  end
end
