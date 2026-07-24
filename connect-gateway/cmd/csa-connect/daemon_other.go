//go:build !linux

package main

import "errors"

func startDaemon(configPath string) error {
	return errors.New("csa-connect start is supported only by the bundled Linux runtime")
}

func stopDaemon(configPath string) error {
	return errors.New("csa-connect stop is supported only by the bundled Linux runtime")
}
