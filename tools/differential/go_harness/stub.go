package main

// strategy.go (copied from port-golang-windows-ime/tip) refers to this type from
// runLocalStrategy; the harness only needs the engine, so a stub is enough.
type tipImpl struct {
	imeComposition string
	imeSelected    int
}
