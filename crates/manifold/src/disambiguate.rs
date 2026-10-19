//! Telling apart rows that read the same: the one rule behind every list of pages (quick switcher, trail strip, ...).
//! Rows that share a label get a qualifier naming what differs between them, never more than that: the host when
//! their hosts differ, the path when their paths do, the query items and fragment that others lack, and `#n` for
//! rows with the same address. A label no other row shares gets none.

use crate::url_parts::UrlParts;
use std::collections::HashMap;

/// What each of `rows` (a label and its url) needs next to its label to be told from the others; `None` for a row
/// whose label is its own. Rows are only compared with rows of the same label. Rows with one address are numbered
/// when they are distinct things (two tabs), not when they are the same page seen twice (`number_twins`).
pub(crate) fn qualifiers(rows: &[(&str, &str)], number_twins: bool) -> Vec<Option<String>> {
    let parts: Vec<UrlParts> = rows.iter().map(|(_, url)| UrlParts::parse(url)).collect();
    // What can differ between visits of one address: each `&` item of the query, then the fragment.
    let items: Vec<Vec<String>> = parts
        .iter()
        .map(|p| {
            let query = p.query.split('&').filter(|s| !s.is_empty()).map(str::to_owned);
            query.chain(p.fragment.map(|f| format!("#{f}"))).collect()
        })
        .collect();
    let mut groups: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, (label, _)) in rows.iter().enumerate() {
        groups.entry(label).or_default().push(i);
    }
    let mut out = vec![None; rows.len()];
    for group in groups.values().filter(|g| g.len() > 1) {
        let first = group[0];
        let hosts_differ = group.iter().any(|&i| parts[i].host() != parts[first].host());
        let paths_differ = group.iter().any(|&i| parts[i].path() != parts[first].path());
        for &i in group {
            let mut address = String::new();
            if hosts_differ {
                address.push_str(&parts[i].host());
            }
            if paths_differ {
                address.push_str(parts[i].path());
            }
            let differs = items[i].iter().filter(|item| !group.iter().all(|&j| items[j].contains(item)));
            let mut q: Vec<String> = Some(address).filter(|a| !a.is_empty()).into_iter().collect();
            q.extend(differs.cloned());
            // Rows with the same address cannot differ in text: number them in list order.
            let twins: Vec<usize> = group.iter().copied().filter(|&j| rows[j].1 == rows[i].1).collect();
            if number_twins && twins.len() > 1 {
                let n = twins.iter().position(|&j| j == i).map_or(1, |p| p + 1);
                q.push(format!("#{n}"));
            }
            out[i] = (!q.is_empty()).then(|| q.join(" "));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(rows: &[(&str, &str)]) -> Vec<Option<String>> {
        qualifiers(rows, true)
    }

    fn some(s: &[&str]) -> Vec<Option<String>> {
        s.iter().map(|s| (!s.is_empty()).then(|| s.to_string())).collect()
    }

    #[test]
    fn a_label_of_its_own_gets_nothing() {
        assert_eq!(q(&[("A", "https://a/"), ("B", "https://b/")]), some(&["", ""]));
    }

    #[test]
    fn same_label_on_other_hosts_names_the_host() {
        let rows = [("Home", "https://www.one.test/"), ("Home", "https://two.test/"), ("Other", "https://one.test/")];
        assert_eq!(q(&rows), some(&["one.test", "two.test", ""]));
    }

    #[test]
    fn same_label_on_one_host_names_the_path_not_the_host() {
        let rows = [("Docs", "https://d.test/a"), ("Docs", "https://d.test/b/c")];
        assert_eq!(q(&rows), some(&["/a", "/b/c"]));
    }

    #[test]
    fn the_same_address_names_what_the_query_and_fragment_change() {
        let rows = [
            ("Results", "https://s.test/find?q=cats&page=1"),
            ("Results", "https://s.test/find?q=cats&page=2"),
            ("Results", "https://s.test/find?q=dogs&page=2#top"),
        ];
        assert_eq!(q(&rows), some(&["q=cats page=1", "q=cats page=2", "q=dogs page=2 #top"]));
    }

    #[test]
    fn identical_rows_are_numbered_in_list_order() {
        let rows = [("about:blank", "about:blank"), ("Other", "https://o/"), ("about:blank", "about:blank")];
        assert_eq!(q(&rows), some(&["#1", "", "#2"]));
    }

    #[test]
    fn a_page_seen_twice_is_not_numbered() {
        let rows = [("T", "https://x.test/a"), ("T", "https://x.test/a")];
        assert_eq!(qualifiers(&rows, false), some(&["", ""]));
    }

    #[test]
    fn a_trailing_slash_is_not_a_difference() {
        assert_eq!(q(&[("E", "https://e.test/"), ("E", "https://e.test")]), some(&["", ""]));
    }

    #[test]
    fn twins_keep_their_number_beside_a_host() {
        let rows = [("T", "https://x.test/a"), ("T", "https://x.test/a"), ("T", "https://y.test/")];
        assert_eq!(q(&rows), some(&["x.test/a #1", "x.test/a #2", "y.test"]));
    }
}
