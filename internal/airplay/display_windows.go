//go:build windows

package airplay

import (
	"fmt"
	"sort"
	"unsafe"

	"golang.org/x/sys/windows"
)

const monitorInfoPrimary = 1

type winRect struct {
	Left   int32
	Top    int32
	Right  int32
	Bottom int32
}

type monitorInfoExW struct {
	Size    uint32
	Monitor winRect
	Work    winRect
	Flags   uint32
	Device  [32]uint16
}

var (
	user32DLL           = windows.NewLazySystemDLL("user32.dll")
	procEnumDisplayMons = user32DLL.NewProc("EnumDisplayMonitors")
	procGetMonitorInfoW = user32DLL.NewProc("GetMonitorInfoW")
)

// ListDisplays enumerates active Win32 monitor rectangles. Sorting by the
// DISPLAY device name matches the normal DXGI output order used by FFmpeg's
// ddagrab filter on a single-adapter system and keeps the picker deterministic.
func ListDisplays() ([]Display, error) {
	var displays []Display
	var callbackErr error

	callback := windows.NewCallback(func(monitor, _ uintptr, _ uintptr, _ uintptr) uintptr {
		var info monitorInfoExW
		info.Size = uint32(unsafe.Sizeof(info))
		ok, _, callErr := procGetMonitorInfoW.Call(
			monitor,
			uintptr(unsafe.Pointer(&info)),
		)
		if ok == 0 {
			callbackErr = fmt.Errorf("GetMonitorInfoW: %w", callErr)
			return 0
		}

		displays = append(displays, Display{
			Name:    windows.UTF16ToString(info.Device[:]),
			X:       int(info.Monitor.Left),
			Y:       int(info.Monitor.Top),
			Width:   int(info.Monitor.Right - info.Monitor.Left),
			Height:  int(info.Monitor.Bottom - info.Monitor.Top),
			Primary: info.Flags&monitorInfoPrimary != 0,
		})
		return 1
	})

	ok, _, callErr := procEnumDisplayMons.Call(0, 0, callback, 0)
	if callbackErr != nil {
		return nil, callbackErr
	}
	if ok == 0 {
		return nil, fmt.Errorf("EnumDisplayMonitors: %w", callErr)
	}
	if len(displays) == 0 {
		return nil, fmt.Errorf("Windows reported no active displays")
	}

	sort.SliceStable(displays, func(i, j int) bool {
		return displays[i].Name < displays[j].Name
	})
	for i := range displays {
		displays[i].Index = i
	}
	return displays, nil
}
