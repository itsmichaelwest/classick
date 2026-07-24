use super::{Fact, HardwareFacts, IpodColour, IpodFamily};

pub const HARDWARE_CATALOGUE_VERSION: u32 = 3;

const ONE_HUNDRED_GB: u64 = 100_000_000_000;
const ONE_HUNDRED_FORTY_GB: u64 = 140_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsbIpodKind {
    Ipod2,
    Ipod3,
    Ipod4,
    Photo,
    Mini,
    Video,
    Unclassified,
    Nano1,
    Nano2,
    Nano3,
    Nano4,
    Nano5,
    Nano6,
    Nano7,
    Classic,
    Shuffle1,
    Shuffle2,
    Shuffle3,
    Shuffle4,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompatibleIpodIdentity {
    pub model_num_str: &'static str,
    pub label: String,
}

pub(crate) fn usb_ipod_kind(pid: u16) -> Option<UsbIpodKind> {
    match pid {
        0x1201 => Some(UsbIpodKind::Ipod3),
        0x1202 => Some(UsbIpodKind::Ipod2),
        0x1203 => Some(UsbIpodKind::Ipod4),
        0x1204 => Some(UsbIpodKind::Photo),
        0x1205 => Some(UsbIpodKind::Mini),
        0x1206..=0x1208 => Some(UsbIpodKind::Unclassified),
        0x1209 => Some(UsbIpodKind::Video),
        0x120A => Some(UsbIpodKind::Nano1),
        0x1260 => Some(UsbIpodKind::Nano2),
        0x1261 => Some(UsbIpodKind::Classic),
        0x1262 => Some(UsbIpodKind::Nano3),
        0x1263 => Some(UsbIpodKind::Nano4),
        0x1265 => Some(UsbIpodKind::Nano5),
        0x1266 => Some(UsbIpodKind::Nano6),
        0x1267 => Some(UsbIpodKind::Nano7),
        0x1300 => Some(UsbIpodKind::Shuffle1),
        0x1301 => Some(UsbIpodKind::Shuffle2),
        0x1302 => Some(UsbIpodKind::Shuffle3),
        0x1303 => Some(UsbIpodKind::Shuffle4),
        _ => None,
    }
}

pub(crate) fn is_known_ipod_usb_product_id(pid: u16) -> bool {
    usb_ipod_kind(pid).is_some()
}

pub(crate) fn family_label_for_usb(pid: u16) -> Option<&'static str> {
    Some(match usb_ipod_kind(pid)? {
        UsbIpodKind::Ipod2 => "iPod (2nd gen)",
        UsbIpodKind::Ipod3 => "iPod (3rd gen)",
        UsbIpodKind::Ipod4 => "iPod (4th gen)",
        UsbIpodKind::Photo => "iPod Photo",
        UsbIpodKind::Mini => "iPod Mini",
        UsbIpodKind::Video => "iPod Video",
        UsbIpodKind::Unclassified => "iPod",
        UsbIpodKind::Nano1 => "iPod Nano (1st gen)",
        UsbIpodKind::Nano2 => "iPod Nano (2nd gen)",
        UsbIpodKind::Nano3 => "iPod Nano (3rd gen)",
        UsbIpodKind::Nano4 => "iPod Nano (4th gen)",
        UsbIpodKind::Nano5 => "iPod Nano (5th gen)",
        UsbIpodKind::Nano6 => "iPod Nano (6th gen)",
        UsbIpodKind::Nano7 => "iPod Nano (7th gen)",
        UsbIpodKind::Classic => "iPod Classic",
        UsbIpodKind::Shuffle1 => "iPod Shuffle (1st gen)",
        UsbIpodKind::Shuffle2 => "iPod Shuffle (2nd gen)",
        UsbIpodKind::Shuffle3 => "iPod Shuffle (3rd gen)",
        UsbIpodKind::Shuffle4 => "iPod Shuffle (4th gen)",
    })
}

pub(crate) fn compatible_identity_from_usb(
    pid: u16,
    capacity_bytes: Option<u64>,
) -> Option<CompatibleIpodIdentity> {
    // Model keys and marketed capacities come from the pinned libgpod 0.8.0
    // `ipod_info_table`. libgpod removes the first letter before lookup, so
    // these fallbacks restore the `M` used by real on-device ModelNumStr values.
    // One white/silver SKU represents each capacity bucket; USB cannot report
    // colour. Later Nano and Shuffle database bundles need separate profiles.
    let kind = usb_ipod_kind(pid)?;
    let gb = capacity_bytes.map(decimal_gigabytes);
    let identity = match kind {
        UsbIpodKind::Ipod2 => {
            capacity_identity(gb, &[(12, "M8737"), (u64::MAX, "M8738")], "iPod (2nd gen)")
        }
        UsbIpodKind::Ipod3 => capacity_identity(
            gb,
            &[
                (13, "M8976"),
                (18, "M8946"),
                (25, "M9244"),
                (35, "M8948"),
                (u64::MAX, "M9245"),
            ],
            "iPod (3rd gen)",
        ),
        UsbIpodKind::Ipod4 => capacity_identity(
            gb,
            &[(23, "M9282"), (32, "M9787"), (u64::MAX, "M9268")],
            "iPod (4th gen)",
        ),
        UsbIpodKind::Photo => capacity_identity(
            gb,
            &[
                (25, "MA079"),
                (35, "M9829"),
                (50, "M9585"),
                (u64::MAX, "M9830"),
            ],
            "iPod Photo",
        ),
        UsbIpodKind::Mini => {
            capacity_identity(gb, &[(5, "M9800"), (u64::MAX, "M9801")], "iPod Mini")
        }
        UsbIpodKind::Video => {
            let (model_num_str, label) = match gb {
                Some(g) if g < 45 => ("MA444", "iPod Video (30GB)"),
                Some(g) if g < 70 => ("MA003", "iPod Video (5th gen, 60GB)"),
                Some(_) => ("MA448", "iPod Video (5.5 gen, 80GB)"),
                None => ("MA444", "iPod Video"),
            };
            CompatibleIpodIdentity {
                model_num_str,
                label: label.to_owned(),
            }
        }
        UsbIpodKind::Nano1 => capacity_identity(
            gb,
            &[(2, "MA350"), (3, "MA004"), (u64::MAX, "MA005")],
            "iPod Nano (1st gen)",
        ),
        UsbIpodKind::Nano2 => capacity_identity(
            gb,
            &[(3, "MA477"), (6, "MA426"), (u64::MAX, "MA497")],
            "iPod Nano (2nd gen)",
        ),
        UsbIpodKind::Nano3 => capacity_identity(
            gb,
            &[(6, "MA978"), (u64::MAX, "MA980")],
            "iPod Nano (3rd gen)",
        ),
        UsbIpodKind::Nano4 => capacity_identity(
            gb,
            &[(6, "MB480"), (12, "MB598"), (u64::MAX, "MB903")],
            "iPod Nano (4th gen)",
        ),
        UsbIpodKind::Classic => {
            let (model_num_str, label) = match capacity_bytes {
                Some(capacity) if capacity < ONE_HUNDRED_GB => {
                    ("MB029", "iPod Classic (1st gen, 80GB)")
                }
                Some(capacity) if capacity < ONE_HUNDRED_FORTY_GB => {
                    ("MB562", "iPod Classic (2nd gen, 120GB)")
                }
                Some(_) => ("MC293", "iPod Classic (160GB)"),
                None => return None,
            };
            CompatibleIpodIdentity {
                model_num_str,
                label: label.to_owned(),
            }
        }
        UsbIpodKind::Nano5
        | UsbIpodKind::Nano6
        | UsbIpodKind::Nano7
        | UsbIpodKind::Unclassified
        | UsbIpodKind::Shuffle1
        | UsbIpodKind::Shuffle2
        | UsbIpodKind::Shuffle3
        | UsbIpodKind::Shuffle4 => return None,
    };
    Some(identity)
}

fn capacity_identity(
    gb: Option<u64>,
    buckets: &[(u64, &'static str)],
    label: &str,
) -> CompatibleIpodIdentity {
    let model_num_str = buckets
        .iter()
        .find(|(upper_bound, _)| gb.is_none_or(|value| value < *upper_bound))
        .map(|(_, model)| *model)
        .expect("capacity table must end with an unbounded bucket");
    CompatibleIpodIdentity {
        model_num_str,
        label: label.to_owned(),
    }
}

fn decimal_gigabytes(bytes: u64) -> u64 {
    bytes / 1_000_000_000
}

pub fn hardware_facts_from_reported_model_code(model_code: &str) -> Option<HardwareFacts> {
    hardware_facts_from_model_code(model_code, |canonical| {
        if model_code.len() == canonical.len() {
            Fact::reported(canonical)
        } else {
            Fact::decoded(canonical)
        }
    })
}

pub fn hardware_facts_from_decoded_model_code(model_code: &str) -> Option<HardwareFacts> {
    hardware_facts_from_model_code(model_code, Fact::decoded)
}

pub fn hardware_facts_from_usb(pid: u16, capacity_bytes: Option<u64>) -> HardwareFacts {
    let capacity = capacity_bytes.map(Fact::reported);
    let decoded = match usb_ipod_kind(pid) {
        Some(UsbIpodKind::Classic) => {
            Some((IpodFamily::Classic, classic_generation(capacity_bytes)))
        }
        Some(UsbIpodKind::Nano1) => decoded_usb(IpodFamily::Nano, "1"),
        Some(UsbIpodKind::Nano2) => decoded_usb(IpodFamily::Nano, "2"),
        Some(UsbIpodKind::Nano3) => decoded_usb(IpodFamily::Nano, "3"),
        Some(UsbIpodKind::Nano4) => decoded_usb(IpodFamily::Nano, "4"),
        Some(UsbIpodKind::Nano5) => decoded_usb(IpodFamily::Nano, "5"),
        Some(UsbIpodKind::Nano6) => decoded_usb(IpodFamily::Nano, "6"),
        Some(UsbIpodKind::Nano7) => decoded_usb(IpodFamily::Nano, "7"),
        Some(UsbIpodKind::Mini) => decoded_usb_family(IpodFamily::Mini),
        Some(UsbIpodKind::Video) => Some((IpodFamily::Video, video_generation(capacity_bytes))),
        Some(UsbIpodKind::Unclassified) => decoded_usb_family(IpodFamily::Ipod),
        Some(UsbIpodKind::Photo) => decoded_usb_family(IpodFamily::Photo),
        Some(UsbIpodKind::Ipod2) => decoded_usb(IpodFamily::Ipod, "2"),
        Some(UsbIpodKind::Ipod3) => decoded_usb(IpodFamily::Ipod, "3"),
        Some(UsbIpodKind::Ipod4) => decoded_usb(IpodFamily::Ipod, "4"),
        Some(UsbIpodKind::Shuffle1) => decoded_usb(IpodFamily::Shuffle, "1"),
        Some(UsbIpodKind::Shuffle2) => decoded_usb(IpodFamily::Shuffle, "2"),
        Some(UsbIpodKind::Shuffle3) => decoded_usb(IpodFamily::Shuffle, "3"),
        Some(UsbIpodKind::Shuffle4) => decoded_usb(IpodFamily::Shuffle, "4"),
        None => None,
    };

    match decoded {
        Some((family, generation)) => HardwareFacts {
            family: Some(Fact::decoded(family)),
            generation,
            capacity_bytes: capacity,
            ..HardwareFacts::default()
        },
        None => HardwareFacts {
            capacity_bytes: capacity,
            ..HardwareFacts::default()
        },
    }
}

fn hardware_facts_from_model_code(
    model_code: &str,
    model_fact: impl FnOnce(String) -> Fact<String>,
) -> Option<HardwareFacts> {
    let (canonical, family, generation, colour) = model_row(model_code)?;

    Some(HardwareFacts {
        family: Some(Fact::decoded(family)),
        generation: Some(Fact::decoded(generation.to_owned())),
        model_code: Some(model_fact(canonical)),
        colour: Some(Fact::decoded(colour)),
        ..HardwareFacts::default()
    })
}

fn model_row(model_code: &str) -> Option<(String, IpodFamily, &'static str, IpodColour)> {
    use crate::ffi;

    let (canonical, generation, model) = crate::ipod::device::libgpod_model(model_code)?;
    let (family, generation) = match generation {
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_FIRST => (IpodFamily::Ipod, "1"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_SECOND => (IpodFamily::Ipod, "2"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_THIRD => (IpodFamily::Ipod, "3"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_FOURTH => (IpodFamily::Ipod, "4"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_PHOTO => (IpodFamily::Photo, "1"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_MINI_1 => (IpodFamily::Mini, "1"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_MINI_2 => (IpodFamily::Mini, "2"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_NANO_1 => (IpodFamily::Nano, "1"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_NANO_2 => (IpodFamily::Nano, "2"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_NANO_3 => (IpodFamily::Nano, "3"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_NANO_4 => (IpodFamily::Nano, "4"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_VIDEO_1 => (IpodFamily::Video, "5"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_VIDEO_2 => (IpodFamily::Video, "5.5"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_CLASSIC_1 => (IpodFamily::Classic, "1"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_CLASSIC_2 => (IpodFamily::Classic, "2"),
        ffi::Itdb_IpodGeneration_ITDB_IPOD_GENERATION_CLASSIC_3 => (IpodFamily::Classic, "3"),
        _ => return None,
    };
    let colour = match model {
        ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_REGULAR
        | ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_COLOR
        | ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_NANO_WHITE
        | ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_VIDEO_WHITE => IpodColour::White,
        ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_REGULAR_U2
        | ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_COLOR_U2
        | ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_VIDEO_U2 => IpodColour::BlackRed,
        ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_NANO_BLACK
        | ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_VIDEO_BLACK
        | ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_CLASSIC_BLACK => IpodColour::Black,
        ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_MINI
        | ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_NANO_SILVER
        | ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_CLASSIC_SILVER => IpodColour::Silver,
        ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_MINI_BLUE
        | ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_NANO_BLUE => IpodColour::Blue,
        ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_MINI_GREEN
        | ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_NANO_GREEN => IpodColour::Green,
        ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_MINI_PINK
        | ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_NANO_PINK => IpodColour::Pink,
        ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_NANO_RED => IpodColour::Red,
        ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_NANO_YELLOW => IpodColour::Yellow,
        ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_NANO_PURPLE => IpodColour::Purple,
        ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_NANO_ORANGE => IpodColour::Orange,
        ffi::Itdb_IpodModel_ITDB_IPOD_MODEL_MINI_GOLD => IpodColour::Gold,
        _ => return None,
    };
    Some((canonical, family, generation, colour))
}

fn classic_generation(capacity_bytes: Option<u64>) -> Option<Fact<String>> {
    match capacity_bytes {
        Some(capacity) if capacity < ONE_HUNDRED_GB => Some(Fact::inferred("1".to_owned())),
        Some(capacity) if capacity < ONE_HUNDRED_FORTY_GB => Some(Fact::inferred("2".to_owned())),
        _ => None,
    }
}

fn video_generation(capacity_bytes: Option<u64>) -> Option<Fact<String>> {
    match capacity_bytes.map(decimal_gigabytes) {
        Some(capacity) if capacity >= 70 => Some(Fact::inferred("5.5".to_owned())),
        Some(capacity) if capacity >= 45 => Some(Fact::inferred("5".to_owned())),
        _ => None,
    }
}

fn decoded_usb(family: IpodFamily, generation: &str) -> Option<(IpodFamily, Option<Fact<String>>)> {
    Some((family, Some(Fact::decoded(generation.to_owned()))))
}

fn decoded_usb_family(family: IpodFamily) -> Option<(IpodFamily, Option<Fact<String>>)> {
    Some((family, None))
}
