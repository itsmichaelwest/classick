use crate::ipod::db::Tags;
use crate::transcode::{ProbeOutput, ProbeTags};

/// Convert an ffprobe-derived `ProbeOutput` into the libgpod-facing `Tags`
/// struct. Pulls track / disc / year out of the messy "n/N" string formats
/// using the small parse helpers below.
pub fn tags_from_probe(p: &ProbeOutput) -> Tags {
    let duration_ms = p.duration_ms;
    let pt: &ProbeTags = match &p.format.tags {
        Some(t) => t,
        None => return Tags { duration_ms, ..Tags::default() },
    };

    let track_nr = pt.track.as_deref().and_then(|s| parse_int_first_field(s));
    let tracks_from_total = pt.track_total.as_deref().and_then(|s| s.trim().parse().ok());
    let tracks_from_slash = pt.track.as_deref().and_then(parse_int_second_field);
    let tracks = tracks_from_total.or(tracks_from_slash);

    let disc_nr = pt.disc.as_deref().and_then(|s| parse_int_first_field(s));
    let discs_from_total = pt.disc_total.as_deref().and_then(|s| s.trim().parse().ok());
    let discs_from_slash = pt.disc.as_deref().and_then(parse_int_second_field);
    let discs = discs_from_total.or(discs_from_slash);

    let year = pt.date.as_deref().and_then(parse_year);

    Tags {
        title: pt.title.clone(),
        artist: pt.artist.clone(),
        album: pt.album.clone(),
        album_artist: pt.album_artist.clone(),
        genre: pt.genre.clone(),
        composer: pt.composer.clone(),
        year,
        track_nr,
        tracks,
        disc_nr,
        discs,
        duration_ms,
    }
}

/// "9/12" -> Some(9). "9" -> Some(9). "" / garbage -> None.
pub(crate) fn parse_int_first_field(s: &str) -> Option<i32> {
    s.split('/').next()?.trim().parse().ok()
}

/// "9/12" -> Some(12). "9" -> None.
pub(crate) fn parse_int_second_field(s: &str) -> Option<i32> {
    s.split('/').nth(1)?.trim().parse().ok()
}

pub(crate) fn parse_year(s: &str) -> Option<i32> {
    s.split('-').next()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_int_first_field_handles_slash_and_lone() {
        assert_eq!(parse_int_first_field("9/12"), Some(9));
        assert_eq!(parse_int_first_field("9"), Some(9));
        assert_eq!(parse_int_first_field(""), None);
        assert_eq!(parse_int_first_field("abc"), None);
    }

    #[test]
    fn parse_int_second_field_only_returns_after_slash() {
        assert_eq!(parse_int_second_field("9/12"), Some(12));
        assert_eq!(parse_int_second_field("9"), None);
    }

    #[test]
    fn parse_year_handles_iso_date_and_lone_year() {
        assert_eq!(parse_year("2002-09-24"), Some(2002));
        assert_eq!(parse_year("2002"), Some(2002));
        assert_eq!(parse_year(""), None);
    }
}
