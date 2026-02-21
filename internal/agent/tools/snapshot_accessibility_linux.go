//go:build linux

package tools

import (
	"fmt"
	"os/exec"
	"strconv"
	"strings"
)

// getUITreeWithBounds retrieves the accessibility tree for an app with element positions via AT-SPI.
func getUITreeWithBounds(app string, windowBounds Rect) []RawElement {
	if app == "" {
		return nil
	}

	// Check for AT-SPI availability
	checkCmd := exec.Command("python3", "-c", "import gi; gi.require_version('Atspi', '2.0')")
	if checkCmd.Run() != nil {
		fmt.Println("[screenshot:see] AT-SPI not available, skipping accessibility tree")
		return nil
	}

	script := fmt.Sprintf(`
import gi
gi.require_version('Atspi', '2.0')
from gi.repository import Atspi

def get_elements(obj, depth=0, max_depth=5):
    if depth > max_depth or not obj:
        return

    role = obj.get_role_name()
    name = obj.get_name() or ""
    desc = obj.get_description() or ""
    value = ""

    try:
        ti = obj.get_text_iface()
        if ti:
            value = ti.get_text(0, ti.get_character_count()) or ""
    except:
        pass

    try:
        vi = obj.get_value_iface()
        if vi and not value:
            value = str(vi.get_current_value())
    except:
        pass

    actionable = False
    try:
        ai = obj.get_action_iface()
        if ai and ai.get_n_actions() > 0:
            actionable = True
    except:
        pass

    try:
        comp = obj.get_component_iface()
        if comp:
            ext = comp.get_extents(Atspi.CoordType.SCREEN)
            x, y, w, h = ext.x, ext.y, ext.width, ext.height
            if w > 0 and h > 0:
                act = "1" if actionable else "0"
                # Escape pipes in values
                name = name.replace("|", " ")
                desc = desc.replace("|", " ")
                value = value.replace("|", " ")
                print(f"{role}|{name}|{desc}|{value}|{x}|{y}|{w}|{h}|{act}")
    except:
        pass

    for i in range(obj.get_child_count()):
        child = obj.get_child_at_index(i)
        if child:
            get_elements(child, depth + 1, max_depth)

desktop = Atspi.get_desktop(0)
for i in range(desktop.get_child_count()):
    app_obj = desktop.get_child_at_index(i)
    if app_obj and "%s".lower() in (app_obj.get_name() or "").lower():
        for j in range(app_obj.get_child_count()):
            window = app_obj.get_child_at_index(j)
            if window:
                get_elements(window)
        break
`, escapeAtspyPy(app))

	cmd := exec.Command("python3", "-c", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		fmt.Printf("[screenshot:see] AT-SPI error: %v\n", err)
		return nil
	}

	return parseLinuxElements(strings.TrimSpace(string(out)))
}

func parseLinuxElements(output string) []RawElement {
	if output == "" {
		return nil
	}

	var elements []RawElement
	for _, line := range strings.Split(output, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}

		parts := strings.SplitN(line, "|", 9)
		if len(parts) < 9 {
			continue
		}

		x, _ := strconv.Atoi(parts[4])
		y, _ := strconv.Atoi(parts[5])
		w, _ := strconv.Atoi(parts[6])
		h, _ := strconv.Atoi(parts[7])
		actionable := parts[8] == "1"

		elements = append(elements, RawElement{
			Role:        parts[0],
			Title:       parts[1],
			Description: parts[2],
			Value:       parts[3],
			Position:    Rect{X: x, Y: y, Width: w, Height: h},
			Actionable:  actionable,
		})
	}
	return elements
}
