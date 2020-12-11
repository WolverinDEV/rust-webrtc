use std::time::{Instant};

/* Using an interval of 64 since than we don't have to do a rest division instead we can do a binary op.
 * In basics we should be greater than 64 and must be lower than 128 or must not be a multiple of that.
 */
const HISTORY_TIME_SPAN: usize = 64;

#[derive(Debug)]
pub struct ReceiverStats {
    time_base: Instant,

    initialized: bool,

    highest_sequence_received: u16,
    highest_sequence_timestamp: u64,

    bandwidth_current_index: usize,
    bandwidth_payload_history: [usize; HISTORY_TIME_SPAN],
    bandwidth_header_history: [usize; HISTORY_TIME_SPAN],
}

impl ReceiverStats {
    pub(crate) fn new() -> Self {
        ReceiverStats{
            time_base: Instant::now(),

            initialized: false,

            bandwidth_current_index: 0,
            bandwidth_payload_history: [0; HISTORY_TIME_SPAN],
            bandwidth_header_history: [0; HISTORY_TIME_SPAN],

            highest_sequence_received: 0,
            highest_sequence_timestamp: 0
        }
    }

    fn bandwidth_minute(&self, buffer: &[usize]) -> f64 {
        let mut index = self.bandwidth_current_index;
        let mut count = 0;
        let mut sum = 0;
        while count < 60 {
            if buffer[index] > 0 {
                sum += buffer[index];
                count += 1;
            } else {
                /* it's unlikely that we've received nothing in this timespan. */
            }

            index += 1;

            if index == self.bandwidth_current_index {
                break;
            } else if index >= buffer.len() {
                index = 0;

                if self.bandwidth_current_index == 0 {
                    break;
                }
            }
        }

        if count == 0 {
            0 as f64
        } else {
            sum as f64 / count as f64
        }
    }

    fn bandwidth_second(&self, buffer: &[usize]) -> usize {
        if self.bandwidth_current_index > 0 {
            buffer[self.bandwidth_current_index - 1]
        } else {
            buffer[buffer.len() - 1]
        }
    }

    pub fn bandwidth_header_minute(&self) -> f64 {
        self.bandwidth_minute(&self.bandwidth_header_history[..])
    }

    pub fn bandwidth_header_second(&self) -> usize {
        self.bandwidth_second(&self.bandwidth_header_history[..])
    }

    pub fn bandwidth_payload_minute(&self) -> f64 {
        self.bandwidth_minute(&self.bandwidth_payload_history[..])
    }

    pub fn bandwidth_payload_second(&self) -> usize {
        self.bandwidth_second(&self.bandwidth_payload_history[..])
    }

    pub(crate) fn reset(&mut self) {

    }

    pub(crate) fn reset_bandwith(&mut self) {
        self.bandwidth_payload_history.fill(0);
        self.bandwidth_header_history.fill(0);
    }

    pub(crate) fn register_incoming_rtp(&mut self, packet_id: u16, header_length: usize, payload_length: usize) {
        let current_seconds = self.current_seconds();

        if
            !self.initialized ||
            packet_id > self.highest_sequence_received ||
            self.highest_sequence_received > 0xFF00 ||
                current_seconds - self.highest_sequence_timestamp >= 5
        {
            self.highest_sequence_received = packet_id;
        }

        self.initialized = true;

        let bandwidth_index = (current_seconds as u32 % HISTORY_TIME_SPAN as u32) as usize;
        while self.bandwidth_current_index != bandwidth_index {
            self.bandwidth_current_index += 1;

            if self.bandwidth_current_index >= HISTORY_TIME_SPAN {
                self.bandwidth_current_index = 0;
            }

            self.bandwidth_header_history[self.bandwidth_current_index] = 0;
            self.bandwidth_payload_history[self.bandwidth_current_index] = 0;
        }

        self.bandwidth_header_history[self.bandwidth_current_index] += header_length;
        self.bandwidth_payload_history[self.bandwidth_current_index] += payload_length;
    }

    fn current_seconds(&self) -> u64 {
        Instant::now().duration_since(self.time_base)
            .as_secs()
    }
}