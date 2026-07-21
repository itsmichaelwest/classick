use super::{Fact, HardwareFacts, IpodColour, IpodFamily};

pub const HARDWARE_CATALOGUE_VERSION: u32 = 1;

const ONE_HUNDRED_GB: u64 = 100_000_000_000;
const ONE_HUNDRED_FORTY_GB: u64 = 140_000_000_000;

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
    let decoded = match pid {
        0x1261 => Some((IpodFamily::Classic, classic_generation(capacity_bytes))),
        0x1240 => decoded_usb(IpodFamily::Nano, "1"),
        0x1260 => decoded_usb(IpodFamily::Nano, "2"),
        0x1262 => decoded_usb(IpodFamily::Nano, "3"),
        0x1263 => decoded_usb(IpodFamily::Nano, "4"),
        0x1265 => decoded_usb(IpodFamily::Nano, "5"),
        0x1266 => decoded_usb(IpodFamily::Nano, "6"),
        0x1267 => decoded_usb(IpodFamily::Nano, "7"),
        0x1205 => decoded_usb_family(IpodFamily::Mini),
        0x1209 => decoded_usb(IpodFamily::Video, "5"),
        0x1206 => decoded_usb(IpodFamily::Video, "5.5"),
        0x1204 => decoded_usb_family(IpodFamily::Photo),
        0x1202 => decoded_usb_family(IpodFamily::Ipod),
        0x1201 => decoded_usb(IpodFamily::Ipod, "3"),
        0x1203 => decoded_usb(IpodFamily::Ipod, "4"),
        0x1300 => decoded_usb(IpodFamily::Shuffle, "1"),
        0x1301 => decoded_usb(IpodFamily::Shuffle, "2"),
        0x1302 => decoded_usb(IpodFamily::Shuffle, "3"),
        0x1303 => decoded_usb(IpodFamily::Shuffle, "4"),
        _ => None,
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
    let (canonical, generation, colour) = classic_model_row(model_code)?;

    Some(HardwareFacts {
        family: Some(Fact::decoded(IpodFamily::Classic)),
        generation: Some(Fact::decoded(generation.to_owned())),
        model_code: Some(model_fact(canonical.to_owned())),
        colour: Some(Fact::decoded(colour)),
        ..HardwareFacts::default()
    })
}

fn classic_model_row(model_code: &str) -> Option<(&'static str, &'static str, IpodColour)> {
    match model_code.to_ascii_uppercase().as_str() {
        "MB029" | "B029" => Some(("MB029", "1", IpodColour::Silver)),
        "MB147" | "B147" => Some(("MB147", "1", IpodColour::Black)),
        "MB145" | "B145" => Some(("MB145", "1", IpodColour::Silver)),
        "MB150" | "B150" => Some(("MB150", "1", IpodColour::Black)),
        "MB562" | "B562" => Some(("MB562", "2", IpodColour::Silver)),
        "MB565" | "B565" => Some(("MB565", "2", IpodColour::Black)),
        "MC293" | "C293" => Some(("MC293", "3", IpodColour::Silver)),
        "MC297" | "C297" => Some(("MC297", "3", IpodColour::Black)),
        _ => None,
    }
}

fn classic_generation(capacity_bytes: Option<u64>) -> Option<Fact<String>> {
    match capacity_bytes {
        Some(capacity) if capacity < ONE_HUNDRED_GB => Some(Fact::inferred("1".to_owned())),
        Some(capacity) if capacity < ONE_HUNDRED_FORTY_GB => Some(Fact::inferred("2".to_owned())),
        _ => None,
    }
}

fn decoded_usb(family: IpodFamily, generation: &str) -> Option<(IpodFamily, Option<Fact<String>>)> {
    Some((family, Some(Fact::decoded(generation.to_owned()))))
}

fn decoded_usb_family(family: IpodFamily) -> Option<(IpodFamily, Option<Fact<String>>)> {
    Some((family, None))
}
