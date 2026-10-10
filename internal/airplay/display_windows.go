//go:build windows

package airplay

import (
	"fmt"
	"runtime"
	"syscall"
	"unsafe"

	"golang.org/x/sys/windows"
)

const (
	monitorInfoPrimary             = 1
	d3dDriverTypeHardware          = 1
	d3d11CreateVideoSupport        = 0x800
	d3d11SDKVersion                = 7
	dxgiErrorNotFound       uint32 = 0x887a0002

	// dxgi.h inherits three IUnknown methods and four IDXGIObject methods.
	// GetAdapter, EnumOutputs, and GetDesc are each the first added method.
	// https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/shared/dxgi.h
	comQueryInterfaceSlot = 0
	comReleaseSlot        = 2
	dxgiGetAdapterSlot    = 7
	dxgiEnumOutputsSlot   = 7
	dxgiOutputGetDescSlot = 7
)

type winRect struct {
	Left   int32
	Top    int32
	Right  int32
	Bottom int32
}

type winMonitorInfo struct {
	Size    uint32
	Monitor winRect
	Work    winRect
	Flags   uint32
}

// dxgiOutputDesc follows DXGI_OUTPUT_DESC: WCHAR[32], RECT, BOOL, the
// 32-bit DXGI_MODE_ROTATION enum, and a pointer-sized HMONITOR.
type dxgiOutputDesc struct {
	DeviceName         [32]uint16
	DesktopCoordinates winRect
	AttachedToDesktop  int32
	Rotation           uint32
	Monitor            uintptr
}

// Only the first eight slots of the native COM vtables are needed here.
type displayCOMObject struct {
	vtable *[8]uintptr
}

var (
	user32DLL             = windows.NewLazySystemDLL("user32.dll")
	procGetMonitorInfoW   = user32DLL.NewProc("GetMonitorInfoW")
	d3d11DLL              = windows.NewLazySystemDLL("d3d11.dll")
	procD3D11CreateDevice = d3d11DLL.NewProc("D3D11CreateDevice")
	iidDXGIDevice         = windows.GUID{
		Data1: 0x54ec77fa,
		Data2: 0x1377,
		Data3: 0x44e6,
		Data4: [8]byte{0x8c, 0x32, 0x88, 0xfd, 0x5f, 0x44, 0xc8, 0x4c},
	}
)

// ListDisplays enumerates attached outputs of the default hardware D3D11
// device, matching FFmpeg ddagrab's default device and EnumOutputs order.
// Outputs on other adapters are not addressable by that capture backend.
func ListDisplays() ([]Display, error) {
	if err := procD3D11CreateDevice.Find(); err != nil {
		return nil, fmt.Errorf("load D3D11CreateDevice: %w", err)
	}
	if err := procGetMonitorInfoW.Find(); err != nil {
		return nil, fmt.Errorf("load GetMonitorInfoW: %w", err)
	}

	// FFmpeg's d3d11va default uses these same arguments, including video
	// support, with no feature-level override or immediate context requested.
	// https://github.com/FFmpeg/FFmpeg/blob/master/libavutil/hwcontext_d3d11va.c
	var device *displayCOMObject
	hr, _, _ := procD3D11CreateDevice.Call(
		0, d3dDriverTypeHardware, 0, d3d11CreateVideoSupport, 0, 0,
		d3d11SDKVersion, uintptr(unsafe.Pointer(&device)), 0, 0,
	)
	device, err := checkedDisplayCOMObject("D3D11CreateDevice", hr, device)
	if err != nil {
		return nil, err
	}
	defer device.release()

	dxgiDevice, err := device.queryDXGIDevice()
	if err != nil {
		return nil, err
	}
	defer dxgiDevice.release()
	adapter, err := dxgiDevice.getAdapter()
	if err != nil {
		return nil, err
	}
	defer adapter.release()

	return listDXGIDisplays(adapter, monitorIsPrimary)
}

func (object *displayCOMObject) release() {
	if object != nil {
		syscall.SyscallN(object.vtable[comReleaseSlot], uintptr(unsafe.Pointer(object)))
		runtime.KeepAlive(object)
	}
}

func checkedDisplayCOMObject(method string, hr uintptr, object *displayCOMObject) (*displayCOMObject, error) {
	if err := displayHRESULTError(method, hr); err != nil {
		object.release()
		return nil, err
	}
	if object == nil {
		return nil, fmt.Errorf("%s succeeded without returning a COM object", method)
	}
	return object, nil
}

func displayHRESULTError(method string, hr uintptr) error {
	// HRESULT is a signed 32-bit value even on 64-bit Windows. The syscall
	// last-error result is unrelated to the return status of COM methods.
	if int32(uint32(hr)) < 0 {
		return fmt.Errorf("%s: HRESULT 0x%08x", method, uint32(hr))
	}
	return nil
}

func (object *displayCOMObject) queryDXGIDevice() (*displayCOMObject, error) {
	var device *displayCOMObject
	hr, _, _ := syscall.SyscallN(object.vtable[comQueryInterfaceSlot],
		uintptr(unsafe.Pointer(object)), uintptr(unsafe.Pointer(&iidDXGIDevice)),
		uintptr(unsafe.Pointer(&device)))
	runtime.KeepAlive(object)
	return checkedDisplayCOMObject("ID3D11Device.QueryInterface(IDXGIDevice)", hr, device)
}

func (object *displayCOMObject) getAdapter() (*displayCOMObject, error) {
	var adapter *displayCOMObject
	hr, _, _ := syscall.SyscallN(object.vtable[dxgiGetAdapterSlot],
		uintptr(unsafe.Pointer(object)), uintptr(unsafe.Pointer(&adapter)))
	runtime.KeepAlive(object)
	return checkedDisplayCOMObject("IDXGIDevice.GetAdapter", hr, adapter)
}

func listDXGIDisplays(adapter *displayCOMObject, isPrimary func(uintptr) (bool, error)) ([]Display, error) {
	var displays []Display
	for index := uint32(0); ; index++ {
		var output *displayCOMObject
		hr, _, _ := syscall.SyscallN(adapter.vtable[dxgiEnumOutputsSlot],
			uintptr(unsafe.Pointer(adapter)), uintptr(index), uintptr(unsafe.Pointer(&output)))
		runtime.KeepAlive(adapter)
		if uint32(hr) == dxgiErrorNotFound {
			output.release()
			break
		}
		output, err := checkedDisplayCOMObject(fmt.Sprintf("IDXGIAdapter.EnumOutputs(%d)", index), hr, output)
		if err != nil {
			return nil, err
		}

		var desc dxgiOutputDesc
		hr, _, _ = syscall.SyscallN(output.vtable[dxgiOutputGetDescSlot],
			uintptr(unsafe.Pointer(output)), uintptr(unsafe.Pointer(&desc)))
		output.release()
		if err := displayHRESULTError(fmt.Sprintf("IDXGIOutput.GetDesc(%d)", index), hr); err != nil {
			return nil, err
		}
		if desc.AttachedToDesktop == 0 {
			continue
		}
		primary, err := isPrimary(desc.Monitor)
		if err != nil {
			return nil, fmt.Errorf("DXGI output %d primary monitor: %w", index, err)
		}
		displays = append(displays, Display{
			Index:   int(index),
			Name:    windows.UTF16ToString(desc.DeviceName[:]),
			X:       int(desc.DesktopCoordinates.Left),
			Y:       int(desc.DesktopCoordinates.Top),
			Width:   int(desc.DesktopCoordinates.Right - desc.DesktopCoordinates.Left),
			Height:  int(desc.DesktopCoordinates.Bottom - desc.DesktopCoordinates.Top),
			Primary: primary,
		})
	}
	if len(displays) == 0 {
		return nil, fmt.Errorf("default D3D11 adapter reported no attached desktop outputs")
	}
	return displays, nil
}

func monitorIsPrimary(monitor uintptr) (bool, error) {
	var info winMonitorInfo
	info.Size = uint32(unsafe.Sizeof(info))
	ok, _, callErr := procGetMonitorInfoW.Call(monitor, uintptr(unsafe.Pointer(&info)))
	if ok == 0 {
		return false, fmt.Errorf("GetMonitorInfoW: %w", callErr)
	}
	return info.Flags&monitorInfoPrimary != 0, nil
}
