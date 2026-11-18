//! Desktop CAT PTT: build the station's keyers from the PTT settings and the
//! radio config, using a serial control line or a CAT command per radio.

use rr_cat::k4::{K4Config, K4Link};
use rr_cat::{CatMonitor, CatPtt, RigConfig, RigModel};
use rr_radio::{KeyMethod, PttSettings, PttSink, StationPtt};

use crate::rigs::{self, Rig, Rigs};

/// A key sink of either kind.
type Sink = Box<dyn PttSink + Send>;

/// The CAT links in use, for status: (radio number, monitor).
pub type CatLinks = Vec<(u8, CatMonitor)>;

/// Convert the stored settings to the CAT library's form.
fn to_config(r: &Rig) -> Result<RigConfig, String> {
    rigs::validate(r)?;
    let k = &r.k4;
    Ok(RigConfig {
        model: RigModel::K4,
        k4: K4Config {
            link: if k.link == "tcp" { K4Link::Tcp } else { K4Link::Serial },
            serial_port: k.serial_port.trim().to_owned(),
            host: k.host.trim().to_owned(),
            port: k.port,
            password: k.password.clone(),
        },
    })
}

fn cat_sink(radio: u8, rig: &Rig, links: &mut CatLinks) -> Result<Sink, String> {
    let cfg = to_config(rig).map_err(|e| format!("radio {radio} CAT: {e}"))?;
    let ptt = CatPtt::open(&cfg).map_err(|e| format!("radio {radio} CAT: {e}"))?;
    links.push((radio, ptt.monitor()));
    Ok(Box::new(ptt))
}

/// Open the keyers: radio 1, and radio 2 when `two` (a two-radio station).
pub fn open_keyers(ptt: &PttSettings, two: bool, rigs: &Rigs) -> Result<(StationPtt, Option<StationPtt>, CatLinks), String> {
    let s1 = ptt.station.clone().sanitised();
    let s2 = &ptt.station2;
    let mut links = CatLinks::new();
    if two && s1.method == KeyMethod::Serial && s2.method == KeyMethod::Serial {
        // Both on serial lines, possibly the two lines of one port.
        let (a, b) = rr_radio::serial::open_stations(&s1, s2)?;
        return Ok((a, Some(b), links));
    }
    let sink1: Sink = match s1.method {
        KeyMethod::Serial => Box::new(rr_radio::serial::serial_sink(&s1.port, s1.key_line, s1.active_low)?),
        KeyMethod::Cat => cat_sink(1, &rigs.radio1, &mut links)?,
    };
    let sink2: Option<Sink> = match (two, s2.method) {
        (false, _) => None,
        (true, KeyMethod::Serial) => Some(Box::new(rr_radio::serial::serial_sink(&s2.port, s2.key_line, s2.active_low)?)),
        (true, KeyMethod::Cat) => Some(cat_sink(2, &rigs.radio2, &mut links)?),
    };
    let k1 = StationPtt::spawn(sink1, &s1).map_err(|e| format!("start Station PTT: {e}"))?;
    let k2 = match sink2 {
        Some(s) => Some(StationPtt::spawn(s, &s1).map_err(|e| format!("start Station PTT: {e}"))?),
        None => None,
    };
    Ok((k1, k2, links))
}

/// Connect to a radio and read its identity, without keying it.
pub fn test(rig: &Rig) -> Result<String, String> {
    rr_cat::test_connection(&to_config(rig)?)
}

/// The state of each CAT link, for the UI.
pub fn status(links: &CatLinks) -> Vec<crate::status::CatDto> {
    links
        .iter()
        .map(|(radio, m)| {
            let s = m.status();
            crate::status::CatDto { radio: *radio, model: s.model, connected: s.connected, error: s.error, keyed: s.keyed, radio_tx: s.radio_tx }
        })
        .collect()
}
