package memory

import "testing"

func TestNormalizeMemoryKey(t *testing.T) {
	tests := []struct {
		input string
		want  string
	}{
		{"Code_Style", "code-style"},
		{"preferences/code_style", "preferences/code-style"},
		{"Preference/Code-Style", "preference/code-style"},
		{"  user/name  ", "user/name"},
		{"style/humor--dry", "style/humor-dry"},
		{"artifact//landing-page", "artifact/landing-page"},
		{"PERSON/Sarah", "person/sarah"},
		{"user name", "user-name"},
		{"-leading-trailing-", "leading-trailing"},
		{"already-normalized", "already-normalized"},
		{"", ""},
	}

	for _, tt := range tests {
		got := NormalizeMemoryKey(tt.input)
		if got != tt.want {
			t.Errorf("NormalizeMemoryKey(%q) = %q, want %q", tt.input, got, tt.want)
		}
	}
}
