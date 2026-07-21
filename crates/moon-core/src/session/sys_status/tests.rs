use super::*;

#[test]
fn parses_cpu_line() {
    let mut s = CoreSysStatus::default();
    let line = "CPU auto report [Avg] stored to C:\\x\\log Moment: 97.8% Avg: 33.7%";
    assert!(s.parse_line(line, 1000));
    assert_eq!(s.cpu_moment, Some(97.8));
    assert_eq!(s.cpu_avg, Some(33.7));
    assert_eq!(s.cpu_ms, 1000);
    assert_eq!(s.mem_ms, 0);
}

#[test]
fn parses_memory_line() {
    let mut s = CoreSysStatus::default();
    let line = "[Memory] UsedMem App: 641  Sys: 669  FreeMem Phys: 45 Page: 773 ";
    assert!(s.parse_line(line, 2000));
    assert_eq!(s.mem_app_mb, Some(641));
    assert_eq!(s.mem_sys_mb, Some(669));
    assert_eq!(s.free_phys_mb, Some(45));
    assert_eq!(s.free_page_mb, Some(773));
    assert_eq!(s.mem_ms, 2000);
}

#[test]
fn ignores_unrelated_lines() {
    let mut s = CoreSysStatus::default();
    assert!(!s.parse_line("Srv: Sent 451 strategies to clients", 3000));
    assert!(s.is_empty());
}
