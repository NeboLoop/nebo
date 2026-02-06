# Homebrew formula for Nebo
# To install: brew install nebolabs/tap/nebo
# Or: brew tap nebolabs/tap && brew install nebo
#
# NOTE: The canonical formula lives in github.com/nebolabs/homebrew-tap
# This copy is for reference only.

class Nebo < Formula
  desc "AI agent with web UI - your personal AI companion"
  homepage "https://github.com/nebolabs/nebo"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/nebolabs/nebo/releases/download/v#{version}/nebo-darwin-arm64"
      sha256 "2ff792fdfb57a1471eea2672f295ffb29c4e1769256d990d9dce9ddaf9107f94"

      def install
        bin.install "nebo-darwin-arm64" => "nebo"
      end
    end

    on_intel do
      url "https://github.com/nebolabs/nebo/releases/download/v#{version}/nebo-darwin-amd64"
      sha256 "5d900140c26f4cf5826c0942de4a024d507637ea4b724660165ba2419dd27f2e"

      def install
        bin.install "nebo-darwin-amd64" => "nebo"
      end
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/nebolabs/nebo/releases/download/v#{version}/nebo-linux-arm64"
      sha256 "d979b4c9d7ca58db5eeb7090aa1f63dbca310e5c64fea6ef2125b9801d0ee946"

      def install
        bin.install "nebo-linux-arm64" => "nebo"
      end
    end

    on_intel do
      url "https://github.com/nebolabs/nebo/releases/download/v#{version}/nebo-linux-amd64"
      sha256 "7176c90483293f070f4adc157727aafbda1cf41303be4dab91ba46f57e3d54bd"

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

      Data is stored in platform-standard locations:
        macOS:  ~/Library/Application Support/Nebo/
        Linux:  ~/.config/nebo/
    EOS
  end

  test do
    assert_match "nebo", shell_output("#{bin}/nebo --help")
  end
end
