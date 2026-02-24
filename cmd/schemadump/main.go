package main

import (
	"encoding/json"
	"fmt"
	"github.com/neboloop/nebo/internal/agent/tools"
)

func main() {
	t := tools.NewNeboLoopTool(nil)
	schema := t.Schema()
	var m map[string]any
	json.Unmarshal(schema, &m)
	out, _ := json.MarshalIndent(m, "", "  ")
	fmt.Println(string(out))
}
