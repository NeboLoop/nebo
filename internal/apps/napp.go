package apps

import (
	"archive/tar"
	"compress/gzip"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
)

const (
	// maxNappMetaFileSize is the max size for non-binary files in a .napp (1MB).
	maxNappMetaFileSize = 1 * 1024 * 1024
	// maxNappBinarySize is the max size for the binary in a .napp (500MB).
	maxNappBinarySize = 500 * 1024 * 1024
	// maxNappUIFileSize is the max size for individual UI files (5MB).
	maxNappUIFileSize = 5 * 1024 * 1024
)

// ExtractNapp extracts a .napp (tar.gz) package to the destination directory.
//
// Security measures:
//   - Path traversal protection (rejects "../" and absolute paths)
//   - Symlink rejection (no symlinks or hard links allowed)
//   - File size limits (500MB binary, 5MB UI files, 1MB metadata files)
//   - Allowlist validation (only expected files: manifest.json, binary, signatures.json, ui/*)
//   - Validates required files are present (manifest.json, binary, signatures.json)
func ExtractNapp(nappPath, destDir string) error {
	f, err := os.Open(nappPath)
	if err != nil {
		return fmt.Errorf("open napp: %w", err)
	}
	defer f.Close()

	gz, err := gzip.NewReader(f)
	if err != nil {
		return fmt.Errorf("gzip reader: %w", err)
	}
	defer gz.Close()

	tr := tar.NewReader(gz)

	hasManifest := false
	hasBinary := false
	hasSignatures := false

	cleanDestDir := filepath.Clean(destDir)

	for {
		header, err := tr.Next()
		if err == io.EOF {
			break
		}
		if err != nil {
			return fmt.Errorf("read tar entry: %w", err)
		}

		// Reject symlinks and hard links
		if header.Typeflag == tar.TypeSymlink || header.Typeflag == tar.TypeLink {
			return fmt.Errorf("symlinks not allowed in .napp: %s", header.Name)
		}

		// Clean the path and reject traversal attempts
		clean := filepath.Clean(header.Name)
		if strings.HasPrefix(clean, "..") || filepath.IsAbs(clean) {
			return fmt.Errorf("path traversal in .napp: %s", header.Name)
		}

		target := filepath.Join(destDir, clean)
		// Ensure target is within destDir (defense in depth)
		if !strings.HasPrefix(filepath.Clean(target), cleanDestDir+string(filepath.Separator)) &&
			filepath.Clean(target) != cleanDestDir {
			return fmt.Errorf("path escape in .napp: %s", header.Name)
		}

		switch header.Typeflag {
		case tar.TypeDir:
			if err := os.MkdirAll(target, 0700); err != nil {
				return fmt.Errorf("create dir %s: %w", clean, err)
			}

		case tar.TypeReg:
			if !isAllowedNappFile(clean) {
				return fmt.Errorf("unexpected file in .napp: %s", clean)
			}

			maxSize := maxSizeForFile(clean)
			if header.Size > maxSize {
				return fmt.Errorf("file too large in .napp: %s (%d bytes, max %d)", clean, header.Size, maxSize)
			}

			if err := os.MkdirAll(filepath.Dir(target), 0700); err != nil {
				return fmt.Errorf("create parent dir for %s: %w", clean, err)
			}

			perm := os.FileMode(0600)
			if clean == "binary" || clean == "app" {
				perm = 0700
			}

			if err := extractFile(tr, target, perm, maxSize); err != nil {
				return fmt.Errorf("extract %s: %w", clean, err)
			}

			switch clean {
			case "manifest.json":
				hasManifest = true
			case "binary", "app":
				hasBinary = true
			case "signatures.json":
				hasSignatures = true
			}
		}
	}

	if !hasManifest {
		return fmt.Errorf("missing manifest.json in .napp")
	}
	if !hasBinary {
		return fmt.Errorf("missing binary in .napp")
	}
	if !hasSignatures {
		return fmt.Errorf("missing signatures.json in .napp")
	}

	return nil
}

// extractFile writes a tar entry to disk with size enforcement.
func extractFile(r io.Reader, target string, perm os.FileMode, maxSize int64) error {
	outFile, err := os.OpenFile(target, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, perm)
	if err != nil {
		return fmt.Errorf("create file: %w", err)
	}

	// Read up to maxSize+1 to detect oversized files even if header lied
	written, err := io.Copy(outFile, io.LimitReader(r, maxSize+1))
	outFile.Close()

	if err != nil {
		os.Remove(target)
		return fmt.Errorf("write file: %w", err)
	}

	if written > maxSize {
		os.Remove(target)
		return fmt.Errorf("file exceeds size limit (%d bytes)", maxSize)
	}

	return nil
}

// isAllowedNappFile returns true if the file path is expected in a .napp package.
func isAllowedNappFile(path string) bool {
	switch path {
	case "manifest.json", "binary", "app", "signatures.json":
		return true
	}
	if strings.HasPrefix(path, "ui/") {
		return true
	}
	return false
}

// maxSizeForFile returns the maximum allowed size for a file in a .napp package.
func maxSizeForFile(path string) int64 {
	switch path {
	case "binary", "app":
		return maxNappBinarySize
	}
	if strings.HasPrefix(path, "ui/") {
		return maxNappUIFileSize
	}
	return maxNappMetaFileSize
}
