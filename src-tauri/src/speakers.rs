//! The PC's sound outputs, for the Speaker setting. Read from the registry that Windows' audio
//! service keeps (no COM): one key per output endpoint, named by its GUID.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Speaker {
    /// The endpoint id wasapisink takes: "{0.0.0.00000000}.{guid}".
    pub id: String,
    /// As Windows shows it: "Speakers (Realtek(R) Audio)".
    pub name: String,
}

/// One endpoint as the registry lists it.
pub struct Endpoint {
    pub guid: String,
    /// DEVICE_STATE_*: 1 is active (plugged in and enabled).
    pub state: u32,
    /// PKEY_Device_DeviceDesc: "Speakers".
    pub description: Option<String>,
    /// PKEY_DeviceInterface_FriendlyName: "Realtek(R) Audio".
    pub adapter: Option<String>,
}

const DEVICE_STATE_ACTIVE: u32 = 1;

/// The outputs that can play now, named like Windows names them, in name order.
pub fn active(endpoints: impl IntoIterator<Item = Endpoint>) -> Vec<Speaker> {
    let mut speakers: Vec<Speaker> = endpoints
        .into_iter()
        .filter(|e| e.state == DEVICE_STATE_ACTIVE)
        .map(|e| {
            let name = match (e.description.as_deref(), e.adapter.as_deref()) {
                (Some(d), Some(a)) => format!("{d} ({a})"),
                (Some(n), None) | (None, Some(n)) => n.to_string(),
                (None, None) => "Sound output".into(),
            };
            Speaker { id: format!("{{0.0.0.00000000}}.{}", e.guid.to_lowercase()), name }
        })
        .collect();
    speakers.sort_by_key(|s| s.name.to_lowercase());
    speakers
}

/// The PC's active sound outputs (empty where there's no such registry).
pub fn list() -> Vec<Speaker> {
    active(registry::endpoints())
}

#[cfg(windows)]
mod registry {
    use super::Endpoint;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };

    const RENDER: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render";
    const DESCRIPTION: &str = "{a45c254e-df1c-4efd-8020-67d146a850e0},2";
    const ADAPTER: &str = "{b3f8fa53-0004-438e-9003-51a46e139bfc},6";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn string(key: HKEY, sub: &str, value: &str) -> Option<String> {
        let mut buf = [0u16; 512];
        let mut len = std::mem::size_of_val(&buf) as u32;
        let (sub, value) = (wide(sub), wide(value));
        let rc = unsafe { RegGetValueW(key, sub.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let chars = (len as usize / 2).saturating_sub(1);
        Some(String::from_utf16_lossy(&buf[..chars])).filter(|s| !s.is_empty())
    }

    fn dword(key: HKEY, sub: &str, value: &str) -> Option<u32> {
        let mut out = 0u32;
        let mut len = 4u32;
        let (sub, value) = (wide(sub), wide(value));
        let rc = unsafe { RegGetValueW(key, sub.as_ptr(), value.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut out as *mut u32).cast(), &mut len) };
        (rc == ERROR_SUCCESS).then_some(out)
    }

    pub fn endpoints() -> Vec<Endpoint> {
        let mut out = Vec::new();
        unsafe {
            let mut render: HKEY = std::ptr::null_mut();
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, wide(RENDER).as_ptr(), 0, KEY_READ, &mut render) != ERROR_SUCCESS {
                return out;
            }
            for i in 0.. {
                let mut name = [0u16; 128];
                let mut len = name.len() as u32;
                let rc = RegEnumKeyExW(render, i, name.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
                if rc != ERROR_SUCCESS {
                    break;
                }
                let guid = String::from_utf16_lossy(&name[..len as usize]);
                let props = format!(r"{guid}\Properties");
                out.push(Endpoint {
                    state: dword(render, &guid, "DeviceState").unwrap_or(0),
                    description: string(render, &props, DESCRIPTION),
                    adapter: string(render, &props, ADAPTER),
                    guid,
                });
            }
            RegCloseKey(render);
        }
        out
    }
}

#[cfg(not(windows))]
mod registry {
    pub fn endpoints() -> Vec<super::Endpoint> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(guid: &str, state: u32, description: Option<&str>, adapter: Option<&str>) -> Endpoint {
        Endpoint { guid: guid.into(), state, description: description.map(Into::into), adapter: adapter.map(Into::into) }
    }

    #[test]
    fn only_active_outputs_are_listed_with_windows_names_and_wasapi_ids() {
        let speakers = active([
            endpoint("{597B88B2-8565-49A2-B30E-691EB83DD31A}", 1, Some("Speakers"), Some("Realtek(R) Audio")),
            endpoint("{8493c34f-9dd4-44d3-93ce-b7170ea303a5}", 8, Some("Headphones"), Some("Realtek(R) Audio")),
            endpoint("{080b4fde-b5d6-436a-87f9-1f06bafa0720}", 1, Some("1 - Monitor"), Some("HD Audio")),
            endpoint("{306c7858-6445-423e-bb1c-c9330354b1a3}", 4, Some("Digital Output"), None),
        ]);
        assert_eq!(
            speakers,
            vec![
                Speaker { id: "{0.0.0.00000000}.{080b4fde-b5d6-436a-87f9-1f06bafa0720}".into(), name: "1 - Monitor (HD Audio)".into() },
                Speaker { id: "{0.0.0.00000000}.{597b88b2-8565-49a2-b30e-691eb83dd31a}".into(), name: "Speakers (Realtek(R) Audio)".into() },
            ]
        );
        assert!(speakers.iter().all(|s| crate::engine::is_endpoint_id(&s.id)));
    }

    #[test]
    fn an_output_missing_part_of_its_name_still_gets_one() {
        let names: Vec<String> = active([
            endpoint("{a}", 1, Some("Speakers"), None),
            endpoint("{b}", 1, None, Some("USB Audio")),
            endpoint("{c}", 1, None, None),
        ])
        .into_iter()
        .map(|s| s.name)
        .collect();
        assert_eq!(names, vec!["Sound output", "Speakers", "USB Audio"]);
    }

    #[cfg(windows)]
    #[test]
    fn reading_this_pcs_outputs_does_not_fail() {
        // Any PC may have none; this only proves the registry read works and ids are well formed.
        for s in list() {
            assert!(crate::engine::is_endpoint_id(&s.id), "{s:?}");
        }
    }
}
