package afv

import (
	"fmt"
	"regexp"
	"strconv"
)

// Violation describes a single fence integrity failure.
type Violation struct {
	FenceID string
	Reason  string
}

// VerifyResult holds the outcome of fence verification.
type VerifyResult struct {
	OK         bool
	Total      int
	Passed     int
	Failed     int
	Violations []Violation
}

var (
	fenceARe = regexp.MustCompile(`\$\$FENCE_A_(\d+)\$\$`)
	fenceBRe = regexp.MustCompile(`\$\$FENCE_B_(\d+)\$\$`)
)

// Verify checks that all fence markers in the context record match the
// checksums stored in the FenceStore. Returns a VerifyResult indicating
// whether the context is structurally intact.
func Verify(store *FenceStore, contextRecord string) *VerifyResult {
	pairs := store.All()
	result := &VerifyResult{
		Total: len(pairs),
	}

	// Extract all A and B values from the context record
	aMatches := fenceARe.FindAllStringSubmatch(contextRecord, -1)
	bMatches := fenceBRe.FindAllStringSubmatch(contextRecord, -1)

	aValues := make(map[int]bool, len(aMatches))
	for _, m := range aMatches {
		if v, err := strconv.Atoi(m[1]); err == nil {
			aValues[v] = true
		}
	}

	bValues := make(map[int]bool, len(bMatches))
	for _, m := range bMatches {
		if v, err := strconv.Atoi(m[1]); err == nil {
			bValues[v] = true
		}
	}

	for _, fp := range pairs {
		hasA := aValues[fp.A]
		hasB := bValues[fp.B]

		if hasA && hasB {
			result.Passed++
			continue
		}

		result.Failed++
		if !hasA && !hasB {
			result.Violations = append(result.Violations, Violation{
				FenceID: fp.ID,
				Reason:  fmt.Sprintf("both markers missing (A=%d, B=%d)", fp.A, fp.B),
			})
		} else if !hasA {
			result.Violations = append(result.Violations, Violation{
				FenceID: fp.ID,
				Reason:  fmt.Sprintf("opening marker missing (A=%d)", fp.A),
			})
		} else {
			result.Violations = append(result.Violations, Violation{
				FenceID: fp.ID,
				Reason:  fmt.Sprintf("closing marker missing (B=%d)", fp.B),
			})
		}
	}

	result.OK = result.Failed == 0
	return result
}
