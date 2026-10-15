pub enum AppEvent {
  PtyOutput { tab_id: u64, bytes: Vec<u8> },
}
