//go:build windows

package airplay

import (
	"errors"
	"reflect"
	"strings"
	"testing"
	"unsafe"

	"golang.org/x/sys/windows"
)

func TestDXGIOutputDescWindowsLayout(t *testing.T) {
	var desc dxgiOutputDesc
	for _, field := range []struct {
		name   string
		offset uintptr
		want   uintptr
	}{
		{"DeviceName", unsafe.Offsetof(desc.DeviceName), 0},
		{"DesktopCoordinates", unsafe.Offsetof(desc.DesktopCoordinates), 64},
		{"AttachedToDesktop", unsafe.Offsetof(desc.AttachedToDesktop), 80},
		{"Rotation", unsafe.Offsetof(desc.Rotation), 84},
		{"Monitor", unsafe.Offsetof(desc.Monitor), 88},
	} {
		if field.offset != field.want {
			t.Errorf("DXGI_OUTPUT_DESC.%s offset = %d, want %d", field.name, field.offset, field.want)
		}
	}
	if got, want := unsafe.Sizeof(desc), uintptr(88)+unsafe.Sizeof(desc.Monitor); got != want {
		t.Errorf("DXGI_OUTPUT_DESC size = %d, want %d", got, want)
	}
	if got := unsafe.Sizeof(winMonitorInfo{}); got != 40 {
		t.Errorf("MONITORINFO size = %d, want 40", got)
	}
}

func newTestDisplayCOMObject(t *testing.T) (*displayCOMObject, *int) {
	t.Helper()
	object := &displayCOMObject{vtable: new([8]uintptr)}
	releases := new(int)
	object.vtable[2] = windows.NewCallback(func(this *displayCOMObject) uintptr {
		if this != object {
			t.Errorf("Release received object %p, want %p", this, object)
		}
		*releases++
		return 0
	})
	return object, releases
}

func newTestDXGIOutput(t *testing.T, desc dxgiOutputDesc, result uint32) (*displayCOMObject, *int) {
	t.Helper()
	object, releases := newTestDisplayCOMObject(t)
	object.vtable[7] = windows.NewCallback(func(this *displayCOMObject, outputDesc *dxgiOutputDesc) uintptr {
		if this != object {
			t.Errorf("GetDesc received object %p, want %p", this, object)
		}
		*outputDesc = desc
		return uintptr(result)
	})
	return object, releases
}

func newTestDXGIAdapter(t *testing.T, outputs []*displayCOMObject, failure uint32) (*displayCOMObject, *[]uint32) {
	t.Helper()
	adapter, _ := newTestDisplayCOMObject(t)
	indices := new([]uint32)
	adapter.vtable[7] = windows.NewCallback(func(this *displayCOMObject, index uint32, output **displayCOMObject) uintptr {
		if this != adapter {
			t.Errorf("EnumOutputs received object %p, want %p", this, adapter)
		}
		*indices = append(*indices, index)
		if failure != 0 {
			return uintptr(failure)
		}
		if int(index) >= len(outputs) {
			return uintptr(dxgiErrorNotFound)
		}
		*output = outputs[index]
		return 0
	})
	return adapter, indices
}

func testDXGIOutputDesc(name string, attached bool, monitor uintptr, rect winRect) dxgiOutputDesc {
	desc := dxgiOutputDesc{Monitor: monitor, DesktopCoordinates: rect}
	copy(desc.DeviceName[:], windows.StringToUTF16(name))
	if attached {
		desc.AttachedToDesktop = 1
	}
	return desc
}

func TestListDXGIDisplaysPreservesNativeOrderAndSparseIndices(t *testing.T) {
	first, firstReleases := newTestDXGIOutput(t,
		testDXGIOutputDesc(`\\.\DISPLAY9`, true, 10, winRect{-1920, -200, 0, 880}), 0)
	unattached, unattachedReleases := newTestDXGIOutput(t,
		testDXGIOutputDesc(`\\.\DISPLAY2`, false, 0, winRect{}), 0)
	third, thirdReleases := newTestDXGIOutput(t,
		testDXGIOutputDesc(`\\.\DISPLAY1`, true, 20, winRect{0, 0, 2560, 1440}), 0)
	adapter, indices := newTestDXGIAdapter(t, []*displayCOMObject{first, unattached, third}, 0)
	var monitors []uintptr
	displays, err := listDXGIDisplays(adapter, func(monitor uintptr) (bool, error) {
		monitors = append(monitors, monitor)
		return monitor == 20, nil
	})
	if err != nil {
		t.Fatal(err)
	}
	want := []Display{
		{Index: 0, Name: `\\.\DISPLAY9`, X: -1920, Y: -200, Width: 1920, Height: 1080},
		{Index: 2, Name: `\\.\DISPLAY1`, Width: 2560, Height: 1440, Primary: true},
	}
	if !reflect.DeepEqual(displays, want) {
		t.Errorf("displays = %#v, want %#v", displays, want)
	}
	if !reflect.DeepEqual(*indices, []uint32{0, 1, 2, 3}) {
		t.Errorf("EnumOutputs indices = %v, want [0 1 2 3]", *indices)
	}
	if !reflect.DeepEqual(monitors, []uintptr{10, 20}) {
		t.Errorf("primary monitor handles = %v, want [10 20]", monitors)
	}
	for i, releases := range []*int{firstReleases, unattachedReleases, thirdReleases} {
		if *releases != 1 {
			t.Errorf("output %d released %d times, want 1", i, *releases)
		}
	}
}

func TestListDXGIDisplaysReturnsHRESULTFailuresAndReleasesOutput(t *testing.T) {
	const failure = uint32(0x80004005)
	t.Run("EnumOutputs", func(t *testing.T) {
		adapter, _ := newTestDXGIAdapter(t, nil, failure)
		_, err := listDXGIDisplays(adapter, func(uintptr) (bool, error) { return false, nil })
		if err == nil || !strings.Contains(err.Error(), "EnumOutputs(0): HRESULT 0x80004005") {
			t.Fatalf("error = %v, want EnumOutputs HRESULT failure", err)
		}
	})
	t.Run("GetDesc", func(t *testing.T) {
		output, releases := newTestDXGIOutput(t, dxgiOutputDesc{}, failure)
		adapter, _ := newTestDXGIAdapter(t, []*displayCOMObject{output}, 0)
		_, err := listDXGIDisplays(adapter, func(uintptr) (bool, error) { return false, nil })
		if err == nil || !strings.Contains(err.Error(), "GetDesc(0): HRESULT 0x80004005") {
			t.Errorf("error = %v, want GetDesc HRESULT failure", err)
		}
		if *releases != 1 {
			t.Errorf("output released %d times, want 1", *releases)
		}
	})
	t.Run("primary monitor", func(t *testing.T) {
		output, releases := newTestDXGIOutput(t, dxgiOutputDesc{AttachedToDesktop: 1}, 0)
		adapter, _ := newTestDXGIAdapter(t, []*displayCOMObject{output}, 0)
		failure := errors.New("monitor disappeared")
		_, err := listDXGIDisplays(adapter, func(uintptr) (bool, error) { return false, failure })
		if !errors.Is(err, failure) {
			t.Errorf("error = %v, want monitor failure", err)
		}
		if *releases != 1 {
			t.Errorf("output released %d times, want 1", *releases)
		}
	})
}

func TestListDXGIDisplaysRequiresAttachedOutputs(t *testing.T) {
	output, releases := newTestDXGIOutput(t, dxgiOutputDesc{}, 0)
	adapter, _ := newTestDXGIAdapter(t, []*displayCOMObject{output}, 0)
	_, err := listDXGIDisplays(adapter, func(uintptr) (bool, error) {
		t.Error("primary query called for unattached output")
		return false, nil
	})
	if err == nil || !strings.Contains(err.Error(), "no attached desktop outputs") {
		t.Errorf("error = %v, want no attached output error", err)
	}
	if *releases != 1 {
		t.Errorf("output released %d times, want 1", *releases)
	}
}

func TestDisplayCOMQueriesUseNativeSlots(t *testing.T) {
	device, deviceReleases := newTestDisplayCOMObject(t)
	dxgiDevice, dxgiDeviceReleases := newTestDisplayCOMObject(t)
	adapter, adapterReleases := newTestDisplayCOMObject(t)
	device.vtable[0] = windows.NewCallback(func(this *displayCOMObject, iid *windows.GUID, result **displayCOMObject) uintptr {
		if this != device || *iid != iidDXGIDevice {
			t.Errorf("QueryInterface received object %p and IID %v", this, *iid)
		}
		*result = dxgiDevice
		return 0
	})
	dxgiDevice.vtable[7] = windows.NewCallback(func(this *displayCOMObject, result **displayCOMObject) uintptr {
		if this != dxgiDevice {
			t.Errorf("GetAdapter received object %p, want %p", this, dxgiDevice)
		}
		*result = adapter
		return 0
	})
	queriedDevice, err := device.queryDXGIDevice()
	if err != nil || queriedDevice != dxgiDevice {
		t.Fatalf("QueryInterface = %p, %v; want %p, nil", queriedDevice, err, dxgiDevice)
	}
	queriedAdapter, err := queriedDevice.getAdapter()
	if err != nil || queriedAdapter != adapter {
		t.Fatalf("GetAdapter = %p, %v; want %p, nil", queriedAdapter, err, adapter)
	}
	queriedAdapter.release()
	queriedDevice.release()
	device.release()
	for _, releases := range []*int{deviceReleases, dxgiDeviceReleases, adapterReleases} {
		if *releases != 1 {
			t.Errorf("COM object released %d times, want 1", *releases)
		}
	}
}

func TestCheckedDisplayCOMObjectHandlesFailuresAndNilResult(t *testing.T) {
	object, releases := newTestDisplayCOMObject(t)
	result, err := checkedDisplayCOMObject("test", 0x80004005, object)
	if result != nil || err == nil || *releases != 1 {
		t.Errorf("failed COM result = %p, %v; releases = %d", result, err, *releases)
	}
	if _, err := checkedDisplayCOMObject("test", 0, nil); err == nil {
		t.Error("successful HRESULT with nil COM object was accepted")
	}
	if err := displayHRESULTError("test", 1); err != nil {
		t.Errorf("nonzero successful HRESULT: %v", err)
	}
}
