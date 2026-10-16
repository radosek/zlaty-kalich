//! Port of the save routine from the original `player.exe` helper.
//!
//! The game saves with `fscommand("exec", "player.exe\t<args>")`. The Flash
//! projector turned that into a command line, and the Delphi helper wrote
//! `kalich.sav` from its `ParamStr`s, which the game later reads back with
//! `loadVariables("kalich.sav", _root)`. Output is byte-identical to the
//! original so saves move freely between this port and the Windows release.

/// One character each, taken in order from the first argument.
const FLAG_KEYS: [&str; 40] = [
    "msdvere",
    "english",
    "zidstate",
    "partituruma",
    "ruzynenav",
    "kosstate",
    "truhla",
    "romano",
    "romstate",
    "certifikat",
    "prase",
    "letenka",
    "hlidachlida",
    "vodkadverestate",
    "klicsebrat",
    "generaljede",
    "pasakstate",
    "fotakstate",
    "obraz",
    "platilslapce",
    "sprejerkastate",
    "sunastate",
    "sunakecystate",
    "knihastate",
    "ropestate",
    "dalklenoty",
    "budhastate",
    "droggang",
    "ozralastate",
    "bednahofstate",
    "hoffmanstate",
    "rozbuskastate",
    "turistastate",
    "fbistate",
    "zetonystate",
    "mnichstate",
    "poz",
    "qvality",
    "ifs",
    "imusic",
];

/// Whole arguments 2..=5.
const VALUE_KEYS: [&str; 4] = ["texts", "prachy", "cf", "pepacf"];

/// Arguments 6.. are inventory slots, written only when present.
const ITEM_SLOTS: usize = 16;

/// Splits arguments the way Delphi's `ParamStr` does: runs of characters
/// `<= ' '` separate them and double quotes group.
pub fn param_str(cmdline: &str) -> Vec<String> {
    let mut params = Vec::new();
    let mut chars = cmdline.chars().peekable();
    loop {
        while chars.next_if(|&c| c <= ' ').is_some() {}
        if chars.peek().is_none() {
            return params;
        }
        let mut param = String::new();
        let mut quoted = false;
        while let Some(&c) = chars.peek() {
            if c <= ' ' && !quoted {
                break;
            }
            chars.next();
            if c == '"' {
                quoted = !quoted;
            } else {
                param.push(c);
            }
        }
        params.push(param);
    }
}

/// Builds `kalich.sav` from the helper's parameters (program name excluded).
pub fn encode(params: &[String]) -> Vec<u8> {
    let mut out = String::new();
    let flags: Vec<char> = params[0].chars().collect();
    for (i, key) in FLAG_KEYS.iter().enumerate() {
        out.push('&');
        out.push_str(key);
        out.push('=');
        // Delphi read past the end of a short string; a missing flag is
        // written empty, which loadVariables treats the same as unset.
        if let Some(c) = flags.get(i) {
            out.push(*c);
        }
    }
    for (i, key) in VALUE_KEYS.iter().enumerate() {
        out.push('&');
        out.push_str(key);
        out.push('=');
        out.push_str(params.get(i + 1).map_or("", String::as_str));
    }
    for slot in 0..ITEM_SLOTS {
        if let Some(value) = params.get(VALUE_KEYS.len() + 1 + slot) {
            out.push_str(&format!("&itt{slot}="));
            out.push_str(value);
        }
    }
    // Delphi wrote the file as an ANSI (cp1250) text file; every value the
    // game sends is ASCII, so UTF-8 bytes are identical.
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_delphi() {
        assert_eq!(param_str("  a\tb  \"c d\"e "), ["a", "b", "c de"]);
        assert!(param_str(" \t ").is_empty());
    }

    #[test]
    fn encodes_like_player_exe() {
        let flags = "0".repeat(36) + "1201";
        let cmd = format!("{flags}\t3\t150\t1951\t12\t&itt0=7\t&itt1=");
        let sav = String::from_utf8(encode(&param_str(&cmd))).unwrap();
        assert!(sav.starts_with("&msdvere=0&english=0&zidstate=0&"));
        assert!(sav.contains("&mnichstate=0&poz=1&qvality=2&ifs=0&imusic=1&texts=3&prachy=150"));
        assert!(sav.ends_with("&cf=1951&pepacf=12&itt0=&itt0=7&itt1=&itt1="));
    }

    #[test]
    fn reproduces_shipped_save_byte_for_byte() {
        // kalich.sav as shipped with the 2002 release.
        let shipped = "&msdvere=0&english=0&zidstate=0&partituruma=0&ruzynenav=0&kosstate=0&truhla=0&romano=0&romstate=0&certifikat=0&prase=0&letenka=0&hlidachlida=1&vodkadverestate=0&klicsebrat=0&generaljede=0&pasakstate=0&fotakstate=0&obraz=0&platilslapce=0&sprejerkastate=0&sunastate=0&sunakecystate=0&knihastate=0&ropestate=0&dalklenoty=0&budhastate=0&droggang=0&ozralastate=0&bednahofstate=0&hoffmanstate=0&rozbuskastate=0&turistastate=0&fbistate=0&zetonystate=0&mnichstate=0&poz=x&qvality=1&ifs=1&imusic=1&texts=1.5&prachy=1000&cf=1951&pepacf=25&itt0=&itt0=25&itt1=&itt1=&itt2=&itt2=&itt3=&itt3=&itt4=&itt4=&itt5=&itt5=&itt6=&itt6=&itt7=&itt7=&itt8=&itt8=&itt9=&itt9=&itt10=&itt10=&itt11=&itt11=&itt12=&itt12=&itt13=&itt13=&itt14=&itt14=&itt15=&itt15=";
        let mut flags = "0".repeat(36);
        flags.replace_range(12..13, "1");
        flags.push_str("x111");
        let items: Vec<String> = (0..16)
            .map(|i| format!("&itt{i}={}", if i == 0 { "25" } else { "" }))
            .collect();
        let cmd = format!("{flags}\t1.5\t1000\t1951\t25\t{}", items.join("\t"));
        assert_eq!(
            String::from_utf8(encode(&param_str(&cmd))).unwrap(),
            shipped
        );
    }
}
