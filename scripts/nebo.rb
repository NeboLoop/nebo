# Homebrew formula for Nebo
# To install: brew install localrivet/tap/nebo
# Or: brew tap localrivet/tap && brew install nebo

class Gobot < Formula
  desc "AI agent with web UI - your personal AI companion"
  homepage "https://github.com/localrivet/nebo"
  version "0.1.0"  # TODO: Update version
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/localrivet/nebo/releases/download/v#{version}/nebo-darwin-arm64"
      sha256 "PLACEHOLDER_SHA256_ARM64"  # TODO: Update after release

      def install
        bin.install "nebo-darwin-arm64" => "nebo"
      end
    end

    on_intel do
      url "https://github.com/localrivet/nebo/releases/download/v#{version}/nebo-darwin-amd64"
      sha256 "PLACEHOLDER_SHA256_AMD64"  # TODO: Update after release

      def install
        bin.install "nebo-darwin-amd64" => "nebo"
      end
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/localrivet/nebo/releases/download/v#{version}/nebo-linux-arm64"
      sha256 "PLACEHOLDER_SHA256_LINUX_ARM64"  # TODO: Update after release

      def install
        bin.install "nebo-linux-arm64" => "nebo"
      end
    end

    on_intel do
      url "https://github.com/localrivet/nebo/releases/download/v#{version}/nebo-linux-amd64"
      sha256 "PLACEHOLDER_SHA256_LINUX_AMD64"  # TODO: Update after release

      def install
        bin.install "nebo-linux-amd64" => "nebo"
      end
    end
  end

  def caveats
    <<~EOS
      Nebo installed successfully!

      To start Nebo:
        nebo

      Then open http://localhost:27895 in your browser.

      First time setup:
        1. Go to http://localhost:27895/setup
        2. Create your admin account
        3. Add API keys in Settings > Providers

      Configuration is stored in ~/.nebo/
    EOS
  end

  test do
    assert_match "Nebo", shell_output("#{bin}/nebo --version")
  end
end
