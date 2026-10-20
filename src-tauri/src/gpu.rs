use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    pub id: String,
    pub name: String,
    pub discrete: bool,
}

pub fn spawn() {
    platform::spawn();
}

pub fn list() -> Vec<GpuInfo> {
    platform::list()
}

pub fn selected_celsius() -> Option<f64> {
    platform::selected_celsius()
}

pub fn set_prefer(id: String) {
    platform::set_prefer(id);
}

#[cfg(windows)]
mod platform {
    use super::GpuInfo;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, Once, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    const NVIDIA: u32 = 0x10DE;
    const AMD: u32 = 0x1002;
    const INTEL: u32 = 0x8086;
    const MICROSOFT: u32 = 0x1414;
    const SOFTWARE: u32 = 2;
    const DISCRETE_BYTES: u64 = 256 * 1024 * 1024;
    const ADL_EDGE: usize = 8;
    const ADL_HOTSPOT: usize = 27;
    const ADL_GFX: usize = 28;

    static CACHE: Mutex<Vec<GpuSample>> = Mutex::new(Vec::new());
    static PREFER: Mutex<String> = Mutex::new(String::new());
    static READY: AtomicBool = AtomicBool::new(false);

    struct GpuSample {
        id: String,
        name: String,
        discrete: bool,
        dedicated: u64,
        celsius: Option<f64>,
    }

    pub fn spawn() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            thread::Builder::new()
                .name("meng-gpu".into())
                .spawn(|| {
                    let mut session = Session::default();
                    loop {
                        let gpus = session.sample();
                        if let Ok(mut cache) = CACHE.lock() {
                            *cache = gpus;
                        }
                        READY.store(true, Ordering::SeqCst);
                        thread::sleep(Duration::from_secs(1));
                    }
                })
                .ok();
        });
    }

    pub fn list() -> Vec<GpuInfo> {
        for _ in 0..40 {
            if READY.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        CACHE
            .lock()
            .map(|gpus| {
                gpus.iter()
                    .map(|gpu| GpuInfo {
                        id: gpu.id.clone(),
                        name: gpu.name.clone(),
                        discrete: gpu.discrete,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn selected_celsius() -> Option<f64> {
        let id = PREFER.lock().ok()?.clone();
        let gpus = CACHE.lock().ok()?;
        pick(&gpus, &id).and_then(|gpu| gpu.celsius)
    }

    pub fn set_prefer(id: String) {
        if let Ok(mut prefer) = PREFER.lock() {
            *prefer = id;
        }
    }

    fn pick<'a>(gpus: &'a [GpuSample], id: &str) -> Option<&'a GpuSample> {
        if !id.is_empty() {
            if let Some(found) = gpus.iter().find(|gpu| gpu.id == id) {
                return Some(found);
            }
        }
        gpus.iter()
            .filter(|gpu| gpu.discrete)
            .max_by_key(|gpu| gpu.dedicated)
            .or_else(|| gpus.first())
    }

    struct Session {
        nv: Option<Nvml>,
        nv_retry: Option<Instant>,
        adl: Option<Adl>,
        adl_retry: Option<Instant>,
    }

    impl Default for Session {
        fn default() -> Self {
            Self {
                nv: None,
                nv_retry: None,
                adl: None,
                adl_retry: None,
            }
        }
    }

    impl Session {
        fn sample(&mut self) -> Vec<GpuSample> {
            let adapters = enumerate_adapters();
            let mut temps = HashMap::new();
            self.ensure_nv();
            self.ensure_adl();
            if let Some(nv) = &self.nv {
                read_nv(nv, &mut temps);
            }
            if let Some(adl) = &self.adl {
                read_adl(adl, &mut temps);
            }
            adapters
                .into_iter()
                .map(|adapter| {
                    let celsius = adapter.pci.and_then(|key| temps.get(&key).copied());
                    GpuSample {
                        id: adapter.id,
                        name: adapter.name,
                        discrete: adapter.discrete,
                        dedicated: adapter.dedicated,
                        celsius,
                    }
                })
                .collect()
        }

        fn ensure_nv(&mut self) {
            if self.nv.is_some() {
                return;
            }
            if self
                .nv_retry
                .is_some_and(|at| at.elapsed() < Duration::from_secs(30))
            {
                return;
            }
            match Nvml::open() {
                Some(nv) => self.nv = Some(nv),
                None => self.nv_retry = Some(Instant::now()),
            }
        }

        fn ensure_adl(&mut self) {
            if self.adl.is_some() {
                return;
            }
            if self
                .adl_retry
                .is_some_and(|at| at.elapsed() < Duration::from_secs(30))
            {
                return;
            }
            match Adl::open() {
                Some(adl) => self.adl = Some(adl),
                None => self.adl_retry = Some(Instant::now()),
            }
        }
    }

    struct Adapter {
        id: String,
        name: String,
        discrete: bool,
        dedicated: u64,
        pci: Option<(u32, u32, u32)>,
    }

    fn enumerate_adapters() -> Vec<Adapter> {
        unsafe {
            let mut factory = std::ptr::null_mut();
            if CreateDXGIFactory1(&IID_FACTORY1, &mut factory) < 0 || factory.is_null() {
                return Vec::new();
            }
            let _factory = Release(factory);
            let enumerate: EnumAdapters1 = com_fn(factory, 12);
            let mut list = Vec::new();
            for index in 0..16 {
                let mut adapter = std::ptr::null_mut();
                if enumerate(factory, index, &mut adapter) < 0 || adapter.is_null() {
                    break;
                }
                let _adapter = Release(adapter);
                let mut desc = std::mem::zeroed::<AdapterDesc1>();
                let get_desc: GetDesc1 = com_fn(adapter, 10);
                if get_desc(adapter, &mut desc) < 0 {
                    continue;
                }
                if desc.flags & SOFTWARE != 0 || desc.vendor_id == MICROSOFT {
                    continue;
                }
                let pci = match pci_address(desc.luid) {
                    Some(pci) if plausible_pci(pci) => Some(pci),
                    Some(_) => continue,
                    None => None,
                };
                let id = match pci {
                    Some((bus, device, function)) => pci_id(bus, device, function),
                    None => format!("luid:{:08x}:{:08x}", desc.luid.low, desc.luid.high as u32),
                };
                let mut name = utf16_fixed(&desc.description);
                if name.is_empty() {
                    name = fallback_name(desc.vendor_id);
                }
                let dedicated = desc.dedicated_video as u64;
                list.push(Adapter {
                    id,
                    name,
                    discrete: is_discrete(desc.vendor_id, dedicated),
                    dedicated,
                    pci,
                });
            }
            dedupe(list)
        }
    }

    fn dedupe(items: Vec<Adapter>) -> Vec<Adapter> {
        let mut out: Vec<Adapter> = Vec::new();
        for item in items {
            if let Some(prev) = out.iter_mut().find(|prev| prev.id == item.id) {
                if item.dedicated > prev.dedicated {
                    *prev = item;
                }
            } else {
                out.push(item);
            }
        }
        out
    }

    fn is_discrete(vendor: u32, dedicated: u64) -> bool {
        matches!(vendor, NVIDIA | AMD | INTEL) && dedicated >= DISCRETE_BYTES
    }

    fn fallback_name(vendor: u32) -> String {
        match vendor {
            NVIDIA => "NVIDIA".to_string(),
            AMD => "AMD".to_string(),
            INTEL => "Intel".to_string(),
            _ => "显卡".to_string(),
        }
    }

    fn plausible_pci((bus, device, function): (u32, u32, u32)) -> bool {
        bus <= 255 && device <= 31 && function <= 7
    }

    fn pci_id(bus: u32, device: u32, function: u32) -> String {
        format!("{bus:02x}:{device:02x}.{function}")
    }

    fn pci_address(luid: Luid) -> Option<(u32, u32, u32)> {
        let api = kmt_api()?;
        unsafe {
            let mut open = OpenFromLuid { luid, adapter: 0 };
            if (api.open)(&mut open) < 0 || open.adapter == 0 {
                return None;
            }
            let mut address = AdapterAddress {
                bus: 0,
                device: 0,
                function: 0,
            };
            let mut query = QueryAdapterInfo {
                adapter: open.adapter,
                ty: 6,
                data: &mut address,
                data_size: std::mem::size_of::<AdapterAddress>() as u32,
            };
            let status = (api.query)(&mut query);
            let mut close = CloseAdapter {
                adapter: open.adapter,
            };
            let _ = (api.close)(&mut close);
            if status < 0 {
                return None;
            }
            Some((address.bus, address.device, address.function))
        }
    }

    struct KmtApi {
        open: OpenLuid,
        query: QueryInfo,
        close: CloseAd,
    }

    type OpenLuid = unsafe extern "system" fn(*mut OpenFromLuid) -> i32;
    type QueryInfo = unsafe extern "system" fn(*mut QueryAdapterInfo) -> i32;
    type CloseAd = unsafe extern "system" fn(*mut CloseAdapter) -> i32;

    fn kmt_api() -> Option<&'static KmtApi> {
        static API: OnceLock<Option<KmtApi>> = OnceLock::new();
        API.get_or_init(|| unsafe {
            let lib = LoadLibraryW(wide("gdi32.dll").as_ptr());
            if lib == 0 {
                return None;
            }
            Some(KmtApi {
                open: load_fn(lib, b"D3DKMTOpenAdapterFromLuid\0")?,
                query: load_fn(lib, b"D3DKMTQueryAdapterInfo\0")?,
                close: load_fn(lib, b"D3DKMTCloseAdapter\0")?,
            })
        })
        .as_ref()
    }

    struct Nvml {
        lib: isize,
        shutdown: NvShutdown,
        count: NvCount,
        handle: NvHandle,
        info: NvInfo,
        temp: NvTemp,
    }

    type NvInit = unsafe extern "C" fn() -> i32;
    type NvShutdown = unsafe extern "C" fn() -> i32;
    type NvCount = unsafe extern "C" fn(*mut u32) -> i32;
    type NvHandle = unsafe extern "C" fn(u32, *mut *mut c_void) -> i32;
    type NvInfo = unsafe extern "C" fn(*mut c_void, *mut NvmlPciInfo) -> i32;
    type NvTemp = unsafe extern "C" fn(*mut c_void, i32, *mut u32) -> i32;

    impl Drop for Nvml {
        fn drop(&mut self) {
            unsafe {
                (self.shutdown)();
                FreeLibrary(self.lib);
            }
        }
    }

    impl Nvml {
        fn open() -> Option<Self> {
            let lib = load_nvml()?;
            unsafe {
                let init = load_fn::<NvInit>(lib, b"nvmlInit_v2\0")
                    .or_else(|| load_fn(lib, b"nvmlInit\0"));
                let Some(init) = init else {
                    FreeLibrary(lib);
                    return None;
                };
                if init() != 0 {
                    FreeLibrary(lib);
                    return None;
                }
                let shutdown = load_fn::<NvShutdown>(lib, b"nvmlShutdown\0");
                let count = load_fn::<NvCount>(lib, b"nvmlDeviceGetCount_v2\0")
                    .or_else(|| load_fn(lib, b"nvmlDeviceGetCount\0"));
                let handle = load_fn::<NvHandle>(lib, b"nvmlDeviceGetHandleByIndex_v2\0")
                    .or_else(|| load_fn(lib, b"nvmlDeviceGetHandleByIndex\0"));
                let info = load_fn::<NvInfo>(lib, b"nvmlDeviceGetPciInfo_v3\0")
                    .or_else(|| load_fn(lib, b"nvmlDeviceGetPciInfo_v2\0"))
                    .or_else(|| load_fn(lib, b"nvmlDeviceGetPciInfo\0"));
                let temp = load_fn::<NvTemp>(lib, b"nvmlDeviceGetTemperature\0");
                let ready = shutdown.is_some()
                    && count.is_some()
                    && handle.is_some()
                    && info.is_some()
                    && temp.is_some();
                if !ready {
                    if let Some(shutdown) = shutdown {
                        shutdown();
                    }
                    FreeLibrary(lib);
                    return None;
                }
                Some(Self {
                    lib,
                    shutdown: shutdown.unwrap(),
                    count: count.unwrap(),
                    handle: handle.unwrap(),
                    info: info.unwrap(),
                    temp: temp.unwrap(),
                })
            }
        }
    }

    fn load_nvml() -> Option<isize> {
        for path in [
            "nvml.dll",
            r"C:\Windows\System32\nvml.dll",
            r"C:\Program Files\NVIDIA Corporation\NVSMI\nvml.dll",
        ] {
            let lib = unsafe { LoadLibraryW(wide(path).as_ptr()) };
            if lib != 0 {
                return Some(lib);
            }
        }
        None
    }

    fn read_nv(nv: &Nvml, temps: &mut HashMap<(u32, u32, u32), f64>) {
        unsafe {
            let mut count = 0u32;
            if (nv.count)(&mut count) != 0 {
                return;
            }
            for index in 0..count.min(16) {
                let mut device = std::ptr::null_mut();
                if (nv.handle)(index, &mut device) != 0 || device.is_null() {
                    continue;
                }
                let mut pci = std::mem::zeroed::<NvmlPciInfo>();
                if (nv.info)(device, &mut pci) != 0 {
                    continue;
                }
                let Some(key) =
                    parse_bus_id(&pci.bus_id).or_else(|| parse_bus_id(&pci.bus_id_legacy))
                else {
                    continue;
                };
                let mut temp = 0u32;
                if (nv.temp)(device, 0, &mut temp) != 0 {
                    continue;
                }
                if (1..=125).contains(&temp) {
                    temps.entry(key).or_insert(temp as f64);
                }
            }
        }
    }

    struct Adl {
        lib: isize,
        ctx: *mut c_void,
        destroy: AdlDestroy,
        num: AdlNum,
        info: AdlInfo,
        pmlog: Option<AdlPmlog>,
        od5: Option<AdlOd5>,
    }

    type AdlCreate = unsafe extern "C" fn(
        unsafe extern "system" fn(i32) -> *mut c_void,
        i32,
        *mut *mut c_void,
    ) -> i32;
    type AdlDestroy = unsafe extern "C" fn(*mut c_void) -> i32;
    type AdlNum = unsafe extern "C" fn(*mut c_void, *mut i32) -> i32;
    type AdlInfo = unsafe extern "C" fn(*mut c_void, *mut AdapterInfo, i32) -> i32;
    type AdlPmlog = unsafe extern "C" fn(*mut c_void, i32, *mut PmLog) -> i32;
    type AdlOd5 = unsafe extern "C" fn(*mut c_void, i32, i32, *mut AdlTemperature) -> i32;

    impl Drop for Adl {
        fn drop(&mut self) {
            unsafe {
                if !self.ctx.is_null() {
                    (self.destroy)(self.ctx);
                }
                FreeLibrary(self.lib);
            }
        }
    }

    impl Adl {
        fn open() -> Option<Self> {
            let lib = load_adl()?;
            unsafe {
                let create = load_fn::<AdlCreate>(lib, b"ADL2_Main_Control_Create\0");
                let destroy = load_fn::<AdlDestroy>(lib, b"ADL2_Main_Control_Destroy\0");
                let (Some(create), Some(destroy)) = (create, destroy) else {
                    FreeLibrary(lib);
                    return None;
                };
                let mut ctx = std::ptr::null_mut();
                if create(adl_alloc, 1, &mut ctx) < 0 || ctx.is_null() {
                    FreeLibrary(lib);
                    return None;
                }
                let num = load_fn::<AdlNum>(lib, b"ADL2_Adapter_NumberOfAdapters_Get\0");
                let info = load_fn::<AdlInfo>(lib, b"ADL2_Adapter_AdapterInfo_Get\0");
                let pmlog = load_fn::<AdlPmlog>(lib, b"ADL2_New_QueryPMLogData_Get\0");
                let od5 = load_fn::<AdlOd5>(lib, b"ADL2_Overdrive5_Temperature_Get\0");
                if let (Some(num), Some(info)) = (num, info) {
                    if pmlog.is_some() || od5.is_some() {
                        return Some(Self {
                            lib,
                            ctx,
                            destroy,
                            num,
                            info,
                            pmlog,
                            od5,
                        });
                    }
                }
                destroy(ctx);
                FreeLibrary(lib);
                None
            }
        }
    }

    fn load_adl() -> Option<isize> {
        for path in ["atiadlxx.dll", r"C:\Windows\System32\atiadlxx.dll"] {
            let lib = unsafe { LoadLibraryW(wide(path).as_ptr()) };
            if lib != 0 {
                return Some(lib);
            }
        }
        None
    }

    unsafe extern "system" fn adl_alloc(size: i32) -> *mut c_void {
        if size <= 0 {
            return std::ptr::null_mut();
        }
        HeapAlloc(GetProcessHeap(), 0, size as usize)
    }

    fn read_adl(adl: &Adl, temps: &mut HashMap<(u32, u32, u32), f64>) {
        unsafe {
            let mut count = 0i32;
            if (adl.num)(adl.ctx, &mut count) != 0 || count <= 0 {
                return;
            }
            let count = count.min(64) as usize;
            let mut infos = Vec::with_capacity(count);
            for _ in 0..count {
                infos.push(std::mem::zeroed::<AdapterInfo>());
            }
            let bytes = (std::mem::size_of::<AdapterInfo>() * count) as i32;
            if (adl.info)(adl.ctx, infos.as_mut_ptr(), bytes) != 0 {
                return;
            }
            for info in &infos {
                if !amd_vendor(info.vendor) {
                    continue;
                }
                let Some(key) = pci_key(info.bus, info.device, info.function) else {
                    continue;
                };
                if temps.contains_key(&key) {
                    continue;
                }
                if let Some(celsius) = adl_temp(adl, info.adapter_index) {
                    temps.insert(key, celsius);
                }
            }
        }
    }

    fn adl_temp(adl: &Adl, index: i32) -> Option<f64> {
        unsafe {
            if let Some(pmlog) = adl.pmlog {
                let mut out = PmLog {
                    size: std::mem::size_of::<PmLog>() as i32,
                    sensors: [Sensor {
                        supported: 0,
                        value: 0,
                    }; 256],
                };
                if pmlog(adl.ctx, index, &mut out) == 0 {
                    for idx in [ADL_EDGE, ADL_HOTSPOT, ADL_GFX] {
                        if let Some(celsius) = sensor_celsius(&out, idx) {
                            return Some(celsius);
                        }
                    }
                }
            }
            if let Some(od5) = adl.od5 {
                let mut temp = AdlTemperature {
                    size: std::mem::size_of::<AdlTemperature>() as i32,
                    temperature: 0,
                };
                if od5(adl.ctx, index, 0, &mut temp) == 0 {
                    let celsius = temp.temperature as f64 / 1000.0;
                    if (1.0..=125.0).contains(&celsius) {
                        return Some(celsius);
                    }
                }
            }
            None
        }
    }

    fn amd_vendor(vendor: i32) -> bool {
        // 部分驱动把 PCI 厂商号 0x1002 存成十进制 1002。
        vendor == AMD as i32 || vendor == 1002
    }

    fn pci_key(bus: i32, device: i32, function: i32) -> Option<(u32, u32, u32)> {
        if bus < 0 || device < 0 || function < 0 {
            return None;
        }
        Some((bus as u32, device as u32, function as u32))
    }

    fn sensor_celsius(log: &PmLog, index: usize) -> Option<f64> {
        let sensor = log.sensors.get(index)?;
        if sensor.supported == 0 {
            return None;
        }
        let value = sensor.value;
        if (1..=125).contains(&value) {
            return Some(value as f64);
        }
        if (1000..=125_000).contains(&value) {
            return Some(value as f64 / 1000.0);
        }
        None
    }

    fn parse_bus_id(raw: &[u8]) -> Option<(u32, u32, u32)> {
        let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
        let text = std::str::from_utf8(&raw[..end]).ok()?.trim();
        if text.is_empty() {
            return None;
        }
        let (body, function) = text.rsplit_once('.')?;
        let mut parts = body.rsplit(':');
        let device = u32::from_str_radix(parts.next()?.trim(), 16).ok()?;
        let bus = u32::from_str_radix(parts.next()?.trim(), 16).ok()?;
        let function = u32::from_str_radix(function.trim(), 16).ok()?;
        Some((bus, device, function))
    }

    fn utf16_fixed(buf: &[u16]) -> String {
        let end = buf.iter().position(|unit| *unit == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end]).trim().to_string()
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn load_fn<T: Copy>(lib: isize, name: &[u8]) -> Option<T> {
        let ptr = GetProcAddress(lib, name.as_ptr());
        if ptr.is_null() {
            None
        } else {
            Some(std::mem::transmute_copy(&ptr))
        }
    }

    unsafe fn com_fn<T: Copy>(obj: *mut u8, index: usize) -> T {
        let vtbl = *(obj as *const *const usize);
        let ptr = *vtbl.add(index);
        std::mem::transmute_copy(&ptr)
    }

    unsafe fn com_release(obj: *mut u8) {
        if obj.is_null() {
            return;
        }
        let release: unsafe extern "system" fn(*mut u8) -> u32 = com_fn(obj, 2);
        release(obj);
    }

    struct Release(*mut u8);

    impl Drop for Release {
        fn drop(&mut self) {
            unsafe { com_release(self.0) }
        }
    }

    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    const IID_FACTORY1: Guid = Guid {
        data1: 0x770a_ae78,
        data2: 0xf26f,
        data3: 0x4dba,
        data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
    };

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Luid {
        low: u32,
        high: i32,
    }

    #[repr(C)]
    struct AdapterDesc1 {
        description: [u16; 128],
        vendor_id: u32,
        device_id: u32,
        sub_sys_id: u32,
        revision: u32,
        dedicated_video: usize,
        dedicated_system: usize,
        shared_system: usize,
        luid: Luid,
        flags: u32,
    }

    type EnumAdapters1 = unsafe extern "system" fn(*mut u8, u32, *mut *mut u8) -> i32;
    type GetDesc1 = unsafe extern "system" fn(*mut u8, *mut AdapterDesc1) -> i32;

    #[repr(C)]
    struct OpenFromLuid {
        luid: Luid,
        adapter: u32,
    }

    #[repr(C)]
    struct AdapterAddress {
        bus: u32,
        device: u32,
        function: u32,
    }

    #[repr(C)]
    struct QueryAdapterInfo {
        adapter: u32,
        ty: u32,
        data: *mut AdapterAddress,
        data_size: u32,
    }

    #[repr(C)]
    struct CloseAdapter {
        adapter: u32,
    }

    #[repr(C)]
    struct NvmlPciInfo {
        bus_id_legacy: [u8; 16],
        domain: u32,
        bus: u32,
        device: u32,
        pci_device_id: u32,
        pci_sub_system_id: u32,
        bus_id: [u8; 32],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct AdapterInfo {
        size: i32,
        adapter_index: i32,
        udid: [u8; 256],
        bus: i32,
        device: i32,
        function: i32,
        vendor: i32,
        adapter_name: [u8; 256],
        display_name: [u8; 256],
        present: i32,
        exist: i32,
        driver_path: [u8; 256],
        driver_path_ext: [u8; 256],
        pnp: [u8; 256],
        os_display_index: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Sensor {
        supported: i32,
        value: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct PmLog {
        size: i32,
        sensors: [Sensor; 256],
    }

    #[repr(C)]
    struct AdlTemperature {
        size: i32,
        temperature: i32,
    }

    const _: () = assert!(std::mem::size_of::<AdapterInfo>() == 1572);
    const _: () = assert!(std::mem::size_of::<PmLog>() == 2052);
    const _: () = assert!(std::mem::size_of::<AdlTemperature>() == 8);
    const _: () = assert!(std::mem::size_of::<NvmlPciInfo>() == 68);

    #[link(name = "dxgi")]
    extern "system" {
        fn CreateDXGIFactory1(riid: *const Guid, factory: *mut *mut u8) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> isize;
        fn GetProcAddress(module: isize, name: *const u8) -> *mut ();
        fn FreeLibrary(module: isize) -> i32;
        fn GetProcessHeap() -> isize;
        fn HeapAlloc(heap: isize, flags: u32, bytes: usize) -> *mut c_void;
    }

    #[cfg(test)]
    mod tests {
        use super::parse_bus_id;

        #[test]
        fn parses_nvml_bus_id() {
            let mut raw = [0u8; 32];
            let text = b"00000000:01:00.0";
            raw[..text.len()].copy_from_slice(text);
            assert_eq!(parse_bus_id(&raw), Some((1, 0, 0)));
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::GpuInfo;

    pub fn spawn() {}

    pub fn list() -> Vec<GpuInfo> {
        Vec::new()
    }

    pub fn selected_celsius() -> Option<f64> {
        None
    }

    pub fn set_prefer(_id: String) {}
}
