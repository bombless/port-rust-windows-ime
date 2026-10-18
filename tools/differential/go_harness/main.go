// Reference harness around the ORIGINAL Go engine (strategy.go, unmodified apart
// from the one-line tie-order pin applied by run.sh). Reads cases from stdin and
// prints one JSON line per case, in the shape the Rust examples print.
//
//   godiff cands   one composition per line      -> ["text|annotation", ...]
//   godiff keys    space separated hex VK codes  -> [[consume, composition, selected, commit|null], ...]
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"strconv"
	"strings"
)

func main() {
	if len(os.Args) != 2 || (os.Args[1] != "cands" && os.Args[1] != "keys") {
		fmt.Fprintln(os.Stderr, "usage: godiff cands|keys < cases")
		os.Exit(2)
	}
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 1<<20), 1<<20)
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	for in.Scan() {
		line := in.Text()
		var result interface{}
		if os.Args[1] == "cands" {
			texts := []string{}
			for _, c := range candidatesFor(line) {
				texts = append(texts, c.Text+"|"+c.Annotation)
			}
			result = texts
		} else {
			steps := [][]interface{}{}
			state := imeState{}
			for _, field := range strings.Fields(line) {
				vk, err := strconv.ParseUint(field, 16, 32)
				if err != nil {
					fmt.Fprintln(os.Stderr, "bad vk:", field)
					os.Exit(2)
				}
				key := keyForVK(uintptr(vk))
				if key == "" { // no mapping: the key belongs to the application
					steps = append(steps, []interface{}{false, state.composition, state.selected, nil})
					continue
				}
				r := reduceIME(state, key)
				state = r.state
				var commit interface{}
				if r.commit != nil {
					commit = *r.commit
				}
				steps = append(steps, []interface{}{r.consume, state.composition, state.selected, commit})
			}
			result = steps
		}
		b, _ := json.Marshal(result)
		fmt.Fprintln(out, string(b))
	}
}
