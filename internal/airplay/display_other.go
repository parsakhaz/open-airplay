//go:build !windows

package airplay

import "fmt"

func ListDisplays() ([]Display, error) {
	return nil, fmt.Errorf("display enumeration is only available on Windows")
}
