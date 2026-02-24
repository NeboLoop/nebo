package devlog

import (
	"fmt"
	"time"
)

// Printf prints a timestamped debug message to stdout.
// Format: "15:04:05.000 [Tag] message\n"
func Printf(format string, args ...any) {
	msg := fmt.Sprintf(format, args...)
	fmt.Printf("%s %s", time.Now().Format("15:04:05.000"), msg)
}
