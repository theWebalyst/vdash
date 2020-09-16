///! Application logic
///!
///! Edit src/custom/app.rs to create a customised fork of logtail-dash
use linemux::MuxedLines;
use std::collections::HashMap;

use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use std::fs::File;
use std::io::{Error, ErrorKind, Write};
use structopt::StructOpt;
use tempfile::NamedTempFile;

use crate::custom::opt::{Opt, MIN_TIMELINE_STEPS};
use crate::shared::util::StatefulList;

pub static DEBUG_WINDOW_NAME: &str = "Debug Window";

pub static ONE_MINUTE_NAME: &str = "1 minute";
pub static ONE_HOUR_NAME: &str = "1 hour";
pub static ONE_DAY_NAME: &str = "1 day";
pub static ONE_TWELTH_NAME: &str = "1 twelth year";
pub static ONE_YEAR_NAME: &str = "1 year";

pub struct App {
	pub opt: Opt,
	pub dash_state: DashState,
	pub monitors: HashMap<String, LogMonitor>,
	pub logfile_with_focus: String,
	pub logfiles: MuxedLines,
	pub logfile_names: Vec<String>,
}

impl App {
	pub async fn new() -> Result<App, std::io::Error> {
		let mut opt = Opt::from_args();

		if opt.files.is_empty() {
			println!("{}: no logfile(s) specified.", Opt::clap().get_name());
			return exit_with_usage("missing logfiles");
		}

		if opt.timeline_steps < MIN_TIMELINE_STEPS {
			println!(
				"Timeline steps number is too small, minimum is {}",
				MIN_TIMELINE_STEPS
			);
			return exit_with_usage("invalid parameter");
		}

		let mut dash_state = DashState::new();
		dash_state.debug_window = opt.debug_window;
		let mut monitors: HashMap<String, LogMonitor> = HashMap::new();
		let mut logfiles = MuxedLines::new()?;
		let mut name_for_focus = String::new();
		let mut logfile_names = Vec::<String>::new();

		let mut parser_output: Option<tempfile::NamedTempFile> = if opt.debug_dashboard {
			dash_state.main_view = DashViewMain::DashDebug;
			opt.files = opt.files[0..1].to_vec();
			let named_file = NamedTempFile::new()?;
			let path = named_file.path();
			let path_str = path
				.to_str()
				.ok_or_else(|| Error::new(ErrorKind::Other, "invalid path"))?;
			opt.files.push(String::from(path_str));
			Some(named_file)
		} else {
			None
		};
		println!("Loading {} files...", opt.files.len());
		for f in &opt.files {
			println!("file: {}", f);
			let mut monitor = LogMonitor::new(&opt, f.to_string(), opt.lines_max);
			if opt.debug_dashboard && monitor.index == 0 {
				if let Some(named_file) = parser_output {
					monitor.metrics.debug_logfile = Some(named_file);
					parser_output = None;
					dash_state.debug_dashboard = true;
				}
			}
			if opt.ignore_existing {
				logfile_names.push(f.to_string());
				monitors.insert(f.to_string(), monitor);
			} else {
				match monitor.load_logfile() {
					Ok(()) => {
						logfile_names.push(f.to_string());
						monitors.insert(f.to_string(), monitor);
					}
					Err(e) => {
						println!("...failed: {}", e);
						return Err(e);
					}
				}
			}

			if name_for_focus.is_empty() {
				name_for_focus = f.to_string();
			}

			match logfiles.add_file(&f).await {
				Ok(_) => (),
				Err(e) => {
					println!("ERROR: {}", e);
					println!(
						"Note: it is ok for the file not to exist, but the file's parent directory must exist."
					);
					return Err(e);
				}
			}
		}

		let mut app = App {
			opt,
			dash_state,
			monitors,
			logfile_with_focus: name_for_focus.clone(),
			logfiles,
			logfile_names,
		};
		app.set_logfile_focus(&name_for_focus);
		Ok(app)
	}

	pub fn get_monitor_with_focus(&mut self) -> Option<(&mut LogMonitor)> {
		match (&mut self.monitors).get_mut(&self.logfile_with_focus) {
			Some(mut monitor) => Some(monitor),
			None => None,
		}
	}

	pub fn set_logfile_focus(&mut self, logfile_name: &String) {
		match self.get_monitor_with_focus() {
			Some(fading_monitor) => {
				fading_monitor.has_focus = false;
				self.logfile_with_focus = String::new();
			}
			None => (),
		}

		if (logfile_name == DEBUG_WINDOW_NAME) {
			self.dash_state.debug_window_has_focus = true;
			self.logfile_with_focus = logfile_name.clone();
			return;
		} else {
			self.dash_state.debug_window_has_focus = false;
		}

		if let Some(focus_monitor) = (&mut self.monitors).get_mut(logfile_name) {
			focus_monitor.has_focus = true;
			self.logfile_with_focus = logfile_name.clone();
		} else {
			error!("Unable to focus UI on: {}", logfile_name);
		};
	}

	pub fn change_focus_next(&mut self) {
		let mut next_i = 0;
		for (i, name) in self.logfile_names.iter().enumerate() {
			if name == &self.logfile_with_focus {
				if i < self.logfile_names.len() - 1 {
					next_i = i + 1;
				}
				break;
			}
		}

		if next_i == 0 && self.opt.debug_window && self.logfile_with_focus != DEBUG_WINDOW_NAME {
			self.set_logfile_focus(&DEBUG_WINDOW_NAME.to_string());
			return;
		}

		let new_focus_name = &self.logfile_names[next_i].to_string();
		self.set_logfile_focus(&new_focus_name);
	}

	pub fn change_focus_previous(&mut self) {
		let len = self.logfile_names.len();
		let mut previous_i = len - 1;
		for (i, name) in self.logfile_names.iter().enumerate() {
			if name == &self.logfile_with_focus {
				if i > 0 {
					previous_i = i - 1;
				}
				break;
			}
		}

		if self.opt.debug_window
			&& previous_i == len - 1
			&& self.logfile_with_focus != DEBUG_WINDOW_NAME
		{
			self.set_logfile_focus(&DEBUG_WINDOW_NAME.to_string());
			return;
		}
		let new_focus_name = &self.logfile_names[previous_i].to_string();
		self.set_logfile_focus(new_focus_name);
	}

	pub fn handle_arrow_up(&mut self) {
		if let Some(monitor) = self.get_monitor_with_focus() {
			do_bracketed_next_previous(&mut monitor.content, false);
		} else if self.opt.debug_window {
			do_bracketed_next_previous(&mut self.dash_state.debug_window_list, false);
		}
	}

	pub fn handle_arrow_down(&mut self) {
		if let Some(monitor) = self.get_monitor_with_focus() {
			do_bracketed_next_previous(&mut monitor.content, true);
		} else if self.opt.debug_window {
			do_bracketed_next_previous(&mut self.dash_state.debug_window_list, true);
		}
	}
}

/// Move selection forward or back without wrapping at start or end
fn do_bracketed_next_previous(list: &mut StatefulList<String>, next: bool) {
	if (next) {
		if let Some(selected) = list.state.selected() {
			if selected != list.items.len() - 1 {
				list.next();
			}
		} else {
			list.previous();
		}
	} else {
		if let Some(selected) = list.state.selected() {
			if selected != 0 {
				list.previous();
			}
		} else {
			list.previous();
		}
	}
}

fn exit_with_usage(reason: &str) -> Result<App, std::io::Error> {
	println!(
		"Try '{} --help' for more information.",
		Opt::clap().get_name()
	);
	return Err(Error::new(ErrorKind::Other, reason));
}

pub struct LogMonitor {
	pub index: usize,
	pub content: StatefulList<String>,
	max_content: usize, // Limit number of lines in content
	pub has_focus: bool,
	pub logfile: String,
	pub metrics: VaultMetrics,
	pub metrics_status: StatefulList<String>,
}

use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT_MONITOR: AtomicUsize = AtomicUsize::new(0);

impl LogMonitor {
	pub fn new(opt: &Opt, f: String, max_lines: usize) -> LogMonitor {
		let index = NEXT_MONITOR.fetch_add(1, Ordering::Relaxed);
		LogMonitor {
			index,
			logfile: f,
			max_content: max_lines,
			metrics: VaultMetrics::new(&opt),
			content: StatefulList::with_items(vec![]),
			has_focus: false,
			metrics_status: StatefulList::with_items(vec![]),
		}
	}

	pub fn load_logfile(&mut self) -> std::io::Result<()> {
		use std::io::{BufRead, BufReader};

		let f = File::open(self.logfile.to_string());
		let f = match f {
			Ok(file) => file,
			Err(_e) => return Ok(()), // It's ok for a logfile not to exist yet
		};

		let f = BufReader::new(f);

		for line in f.lines() {
			let line = line.expect("Unable to read line");
			self.append_to_content(&line)?
		}

		if self.content.items.len() > 0 {
			self
				.content
				.state
				.select(Some(self.content.items.len() - 1));
		}

		Ok(())
	}

	pub fn append_to_content(&mut self, text: &str) -> Result<(), std::io::Error> {
		if self.line_filter(&text) {
			self.metrics.gather_metrics(&text)?;
			self._append_to_content(text)?; // Show in TUI
		}
		Ok(())
	}

	pub fn _append_to_content(&mut self, text: &str) -> Result<(), std::io::Error> {
		self.content.items.push(text.to_string());
		let len = self.content.items.len();
		if len > self.max_content {
			self.content.items = self.content.items.split_off(len - self.max_content);
		} else {
			self.content.state.select(Some(len - 1));
		}
		Ok(())
	}

	// Some logfile lines are too numerous to include so we ignore them
	// Returns true if the line is to be processed
	fn line_filter(&mut self, line: &str) -> bool {
		true
	}
}

use regex::Regex;
lazy_static::lazy_static! {
	// static ref REGEX_ERROR = "The regex failed to compile. This is a bug.";
	static ref LOG_LINE_PATTERN: Regex =
		Regex::new(r"(?P<category>^[A-Z]{4}) (?P<time_string>[^ ]{35}) (?P<source>\[.*\]) (?P<message>.*)").expect("The regex failed to compile. This is a bug.");

	// static ref STATE_PATTERN: Regex =
	//   Regex::new(r"vault.rs .*No. of Elders: (?P<elders>\d+)").expect(REGEX_ERROR);

	// static ref COUNTS_PATTERN: Regex =215

	// Regex::new(r"vault.rs .*No. of Adults: (?P<elders>\d+)").expect(REGEX_ERROR);
}

#[derive(PartialEq)]
pub enum VaultAgebracket {
	Unknown,
	Infant,
	Adult,
	Elder,
}

///! Maintains one or more 'marching bucket' histories for
///! a given metric, each with its own duration and granularity.
///!
///! A BucketSet is used to hold the history of values with
///! a given bucket_duration and maximum number of buckets.
///!
///! A BucketSet begins with a single bucket of fixed
///! duration holding the initial metric value. New buckets
///! are added as time progresses until the number of buckets
///! covers the total duration of the BucketSet. At this
///! point the oldest bucket is removed when a new bucket is
///! added, so that the total duration remains constant and
///! the specified maximum number of buckets is never
///! exceeded.
///!
///! By adding more than one BucketSet, a given metric can be
///! recorded for different durations and with different
///! granularities. E.g. 60 * 1s buckets covers a minute
///! and 60 * 1m buckets covers an hour, and so on.
pub struct TimelineSet {
	name: String,
	bucket_sets: HashMap<&'static str, BucketSet>,
}

pub struct BucketSet {
	pub bucket_time: Option<DateTime<FixedOffset>>,
	pub total_duration: Duration,
	pub bucket_duration: Duration,
	pub max_buckets: usize,
	pub buckets: Vec<u64>,
}

impl TimelineSet {
	pub fn new(name: String) -> TimelineSet {
		TimelineSet {
			name,
			bucket_sets: HashMap::<&'static str, BucketSet>::new(),
		}
	}

	pub fn get_name(&self) -> &String {
		&self.name
	}

	pub fn add_bucket_set(&mut self, name: &'static str, duration: Duration, max_buckets: usize) {
		self
			.bucket_sets
			.insert(name, BucketSet::new(duration, max_buckets));
	}

	pub fn get_bucket_set(&mut self, bucket_set_name: &str) -> Option<&BucketSet> {
		self.bucket_sets.get(bucket_set_name)
	}

	///! Update all bucket_sets with new current time
	///!
	///! Call significantly more frequently than the smallest BucketSet duration
	fn update_current_time(&mut self, new_time: Option<DateTime<FixedOffset>>) {
		for (name, bs) in self.bucket_sets.iter_mut() {
			if let Some(mut bucket_time) = bs.bucket_time {
				if let Some(new_time) = new_time {
					let mut end_time = bucket_time + bs.bucket_duration;

					while end_time.lt(&new_time) {
						// Start new bucket
						bs.bucket_time = Some(end_time);
						bucket_time = end_time;
						end_time = bucket_time + bs.bucket_duration;

						bs.buckets.push(0);
						if bs.buckets.len() > bs.max_buckets {
							bs.buckets.remove(0);
						}
					}
				}
			} else {
				bs.bucket_time = new_time;
			}
		}
	}

	fn increment_value(&mut self) {
		for (name, bs) in self.bucket_sets.iter_mut() {
			let index = bs.buckets.len() - 1;
			bs.buckets[index] += 1;
		}
	}
}

impl BucketSet {
	pub fn new(bucket_duration: Duration, max_buckets: usize) -> BucketSet {
		BucketSet {
			bucket_duration,
			max_buckets,
			total_duration: bucket_duration * max_buckets as i32,

			bucket_time: None,
			buckets: vec![0],
		}
	}
	pub fn set_bucket_value(&mut self, value: u64) {
		let index = self.buckets.len() - 1;
		self.buckets[index] = value;
	}

	pub fn increment_value(&mut self) {
		let index = self.buckets.len() - 1;
		self.buckets[index] += 1;
	}

	pub fn buckets(&self) -> &Vec<u64> {
		&self.buckets
	}

	pub fn buckets_mut(&mut self) -> &mut Vec<u64> {
		&mut self.buckets
	}
}

pub struct VaultMetrics {
	pub vault_started: Option<DateTime<FixedOffset>>,
	pub running_message: Option<String>,
	pub running_version: Option<String>,
	pub category_count: HashMap<String, usize>,
	pub activity_history: Vec<ActivityEntry>,
	pub log_history: Vec<LogEntry>,

	pub puts_timeline: TimelineSet,
	pub gets_timeline: TimelineSet,
	pub errors_timeline: TimelineSet, // TODO add code to collect and display

	pub most_recent: Option<DateTime<FixedOffset>>,
	pub agebracket: VaultAgebracket,
	pub adults: usize,
	pub elders: usize,
	pub activity_gets: u64,
	pub activity_puts: u64,
	pub activity_errors: u64,
	pub activity_other: u64,

	pub debug_logfile: Option<NamedTempFile>,
	parser_output: String,
}

impl VaultMetrics {
	fn new(opt: &Opt) -> VaultMetrics {
		let mut puts_timeline = TimelineSet::new("PUTS".to_string());
		let mut gets_timeline = TimelineSet::new("GETS".to_string());
		let mut errors_timeline = TimelineSet::new("ERRORS".to_string());
		for timeline in [&mut puts_timeline, &mut gets_timeline, &mut errors_timeline].iter_mut() {
			timeline.add_bucket_set(&ONE_MINUTE_NAME, Duration::minutes(1), opt.timeline_steps);
			timeline.add_bucket_set(&ONE_HOUR_NAME, Duration::hours(1), opt.timeline_steps);
			timeline.add_bucket_set(&ONE_DAY_NAME, Duration::days(1), opt.timeline_steps);
			timeline.add_bucket_set(
				&ONE_TWELTH_NAME,
				Duration::days(365 / 12),
				opt.timeline_steps,
			);
			timeline.add_bucket_set(&ONE_YEAR_NAME, Duration::days(365), opt.timeline_steps);
		}

		VaultMetrics {
			// Start
			vault_started: None,
			running_message: None,
			running_version: None,

			// Logfile entries
			activity_history: Vec::<ActivityEntry>::new(),
			log_history: Vec::<LogEntry>::new(),
			most_recent: None,

			// Timelines / Sparklines
			puts_timeline,
			gets_timeline,
			errors_timeline,

			// Counts
			category_count: HashMap::new(),
			activity_gets: 0,
			activity_puts: 0,
			activity_errors: 0,
			activity_other: 0,

			// State (vault)
			agebracket: VaultAgebracket::Infant,

			// State (network)
			adults: 0,
			elders: 0,

			// Debug
			debug_logfile: None,
			parser_output: String::from("-"),
		}
	}

	pub fn agebracket_string(&self) -> String {
		match self.agebracket {
			VaultAgebracket::Infant => "Infant".to_string(),
			VaultAgebracket::Adult => "Adult".to_string(),
			VaultAgebracket::Elder => "Elder".to_string(),
			VaultAgebracket::Unknown => "Unknown".to_string(),
		}
	}

	fn reset_metrics(&mut self) {
		self.agebracket = VaultAgebracket::Infant;
		self.adults = 0;
		self.elders = 0;
		self.activity_gets = 0;
		self.activity_puts = 0;
		self.activity_errors = 0;
		self.activity_other = 0;
	}

	///! Process a line from a SAFE Vault logfile.
	///! May add a LogEntry to the VaultMetrics::log_history vector.
	///! Use a created LogEntry to update metrics.
	pub fn gather_metrics(&mut self, line: &str) -> Result<(), std::io::Error> {
		// For debugging LogEntry::decode()
		let mut parser_result = format!("LogEntry::decode() failed on: {}", line);
		if let Some(mut entry) = LogEntry::decode(line).or_else(|| self.parse_start(line)) {
			if entry.time.is_none() {
				entry.time = self.most_recent;
			} else {
				self.most_recent = entry.time;
			}

			for timeline in &mut [
				&mut self.puts_timeline,
				&mut self.gets_timeline,
				&mut self.errors_timeline,
			]
			.iter_mut()
			{
				timeline.update_current_time(self.most_recent);
			}

			self.parser_output = entry.parser_output.clone();
			self.process_logfile_entry(&entry); // May overwrite self.parser_output
			parser_result = self.parser_output.clone();
			self.log_history.push(entry);

			// TODO Trim log_history
		}

		// --debug-parser - prints parser results for a single logfile
		// to a temp logfile which is displayed in the adjacent window.
		match &self.debug_logfile {
			Some(f) => {
				use std::io::Seek;
				let mut file = f.reopen()?;
				file.seek(std::io::SeekFrom::End(0))?;
				writeln!(file, "{}", &parser_result)?
			}
			None => (),
		};
		Ok(())
	}

	///! Returm a LogEntry and capture metadata for logfile vault start:
	///!    'Running safe-vault v0.24.0'
	pub fn parse_start(&mut self, line: &str) -> Option<LogEntry> {
		let running_prefix = String::from("Running safe-vault ");

		if line.starts_with(&running_prefix) {
			self.running_message = Some(line.to_string());
			self.running_version = Some(line[running_prefix.len()..].to_string());
			self.vault_started = self.most_recent;
			let parser_output = format!(
				"START at {}",
				self
					.most_recent
					.map_or(String::from("None"), |m| format!("{}", m))
			);

			self.reset_metrics();
			return Some(LogEntry {
				logstring: String::from(line),
				category: String::from("START"),
				time: self.most_recent,
				source: String::from(""),
				message: line.to_string(),
				parser_output,
			});
		}

		None
	}

	///! Process a logfile entry
	///! Returns true if the line has been processed and can be discarded
	pub fn process_logfile_entry(&mut self, entry: &LogEntry) -> bool {
		return self.parse_data_response(
			&entry,
			"Responded to our data handlers with: Response { response: Response::",
		) || self.parse_states(&entry);
	}

	///! Update data metrics from a handler response logfile entry
	///! Returns true if the line has been processed and can be discarded
	fn parse_data_response(&mut self, entry: &LogEntry, pattern: &str) -> bool {
		if let Some(mut response_start) = entry.logstring.find(pattern) {
			response_start += pattern.len();
			let mut response = "";

			if let Some(response_end) = entry.logstring[response_start..].find(",") {
				response = entry.logstring.as_str()[response_start..response_start + response_end].as_ref();
				if !response.is_empty() {
					let activity_entry = ActivityEntry::new(entry, response);
					self.parse_activity_counts(&activity_entry);
					self.activity_history.push(activity_entry);
					self.parser_output = format!("vault activity: {}", response);
				}
			}
			if response.is_empty() {
				self.parser_output = format!("failed to parse_data_response: {}", entry.logstring);
			};

			return true;
		};
		return false;
	}

	///! Capture state updates from a logfile entry
	///! Returns true if the line has been processed and can be discarded
	fn parse_states(&mut self, entry: &LogEntry) -> bool {
		let &content = &entry.logstring.as_str();
		if let Some(elders) = self.parse_usize("No. of Elders:", content) {
			self.elders = elders;
			self.parser_output = format!("ELDERS: {}", elders);
			return true;
		};

		if let Some(adults) = self.parse_usize("No. of Adults:", &entry.logstring) {
			self.adults = adults;
			self.parser_output = format!("ADULTS: {}", adults);
			return true;
		};

		if let Some(agebracket) = self
			.parse_word("Vault promoted to ", &entry.logstring)
			.or(self.parse_word("Initializing new Vault as ", &entry.logstring))
		{
			self.agebracket = match agebracket.as_str() {
				"Infant" => VaultAgebracket::Infant,
				"Adult" => VaultAgebracket::Adult,
				"Elder" => VaultAgebracket::Elder,
				_ => {
					self.parser_output = format!("Error, unkown vault agedbracket '{}'", agebracket);
					VaultAgebracket::Unknown
				}
			};
			if self.agebracket == VaultAgebracket::Unknown {
				self.parser_output = format!("Vault agebracket: {}", agebracket);
			} else {
				self.parser_output = format!("FAILED to parse agebracket in: {}", &entry.logstring);
			}
			return true;
		};

		false
	}

	fn parse_usize(&mut self, prefix: &str, content: &str) -> Option<usize> {
		if let Some(position) = content.find(prefix) {
			match content[position + prefix.len()..].parse::<usize>() {
				Ok(value) => return Some(value),
				Err(e) => self.parser_output = format!("failed to parse usize from: '{}'", content),
			}
		}
		None
	}

	fn parse_word(&mut self, prefix: &str, content: &str) -> Option<String> {
		if let Some(mut start) = content.find(prefix) {
			let word: Vec<&str> = content.trim_start().splitn(1, " ").collect();
			if word.len() == 1 {
				return Some(word[0].to_string());
			} else {
				self.parser_output = format!("failed to parse word at: '{}'", &content[start..]);
			}
		}
		None
	}

	///! Counts vault activity in categories GET, PUT and other
	pub fn parse_activity_counts(&mut self, entry: &ActivityEntry) {
		if entry.activity.starts_with("Get") {
			self.count_get();
		} else if entry.activity.starts_with("Mut") {
			self.count_put();
		} else {
			self.activity_other += 1;
		}
	}

	fn count_get(&mut self) {
		self.activity_gets += 1;
		self.gets_timeline.increment_value();
	}

	fn count_put(&mut self) {
		self.activity_puts += 1;
		self.puts_timeline.increment_value();
	}

	fn count_error(&mut self) {
		self.activity_errors += 1;
		self.errors_timeline.increment_value();
	}

	///! TODO
	pub fn parse_logentry_counts(&mut self, entry: &LogEntry) {
		// Categories ('INFO', 'WARN' etc)
		if !entry.category.is_empty() {
			let count = match self.category_count.get(&entry.category) {
				Some(count) => count + 1,
				None => 1,
			};
			self.category_count.insert(entry.category.clone(), count);
		}
	}
}

///! Vault activity for vault activity_history
pub struct ActivityEntry {
	pub activity: String,
	pub logstring: String,
	pub category: String, // First word, "Running", "INFO", "WARN" etc
	pub time: Option<DateTime<FixedOffset>>,
	pub source: String,

	pub parser_output: String,
}

impl ActivityEntry {
	pub fn new(entry: &LogEntry, activity: &str) -> ActivityEntry {
		ActivityEntry {
			activity: activity.to_string(),
			logstring: entry.logstring.clone(),
			category: entry.category.clone(),
			time: entry.time,
			source: entry.source.clone(),

			parser_output: String::from(""),
		}
	}
}

///! Decoded logfile entries for a vault log history
pub struct LogEntry {
	pub logstring: String,
	pub category: String, // First word, "Running", "INFO", "WARN" etc
	pub time: Option<DateTime<FixedOffset>>,
	pub source: String,
	pub message: String,

	pub parser_output: String,
}

impl LogEntry {
	///! Decode vault logfile lines of the form:
	///!    INFO 2020-07-08T19:58:26.841778689+01:00 [src/bin/safe_vault.rs:114]
	///!    WARN 2020-07-08T19:59:18.540118366+01:00 [src/data_handler/idata_handler.rs:744] 552f45..: Failed to get holders metadata from DB
	///!
	pub fn decode(line: &str) -> Option<LogEntry> {
		let mut test_entry = LogEntry {
			logstring: String::from(line),
			category: String::from("test"),
			time: None,
			source: String::from(""),
			message: String::from(""),
			parser_output: String::from("decode()..."),
		};

		if line.is_empty() {
			return None;
		}

		LogEntry::parse_logfile_line(line)
	}

	///! Parse a line of the form:
	///!    INFO 2020-07-08T19:58:26.841778689+01:00 [src/bin/safe_vault.rs:114]
	///!    WARN 2020-07-08T19:59:18.540118366+01:00 [src/data_handler/idata_handler.rs:744] 552f45..: Failed to get holders metadata from DB
	fn parse_logfile_line(line: &str) -> Option<LogEntry> {
		let captures = LOG_LINE_PATTERN.captures(line)?;

		let category = captures.name("category").map_or("", |m| m.as_str());
		let time_string = captures.name("time_string").map_or("", |m| m.as_str());
		let source = captures.name("source").map_or("", |m| m.as_str());
		let message = captures.name("message").map_or("", |m| m.as_str());
		let mut time_str = String::from("None");
		let time = match DateTime::<FixedOffset>::parse_from_rfc3339(time_string) {
			Ok(time) => {
				time_str = format!("{}", time);
				Some(time)
			}
			Err(e) => None,
		};
		let parser_output = format!(
			"c: {}, t: {}, s: {}, m: {}",
			category, time_str, source, message
		);

		Some(LogEntry {
			logstring: String::from(line),
			category: String::from(category),
			time: time,
			source: String::from(source),
			message: String::from(message),
			parser_output,
		})
	}
}

///! Active UI at top level
pub enum DashViewMain {
	DashSummary,
	DashVault,
	DashDebug,
}

pub struct DashState {
	pub main_view: DashViewMain,
	pub active_timeline_name: &'static str,

	// For --debug-window option
	pub debug_window_list: StatefulList<String>,
	pub debug_window: bool,
	pub debug_window_has_focus: bool,
	pub debug_dashboard: bool,
	max_debug_window: usize,
}

impl DashState {
	pub fn new() -> DashState {
		DashState {
			main_view: DashViewMain::DashVault,
			active_timeline_name: ONE_MINUTE_NAME,

			debug_dashboard: false,
			debug_window: false,
			debug_window_has_focus: false,
			debug_window_list: StatefulList::new(),
			max_debug_window: 100,
		}
	}

	pub fn _debug_window(&mut self, text: &str) {
		self.debug_window_list.items.push(text.to_string());
		let len = self.debug_window_list.items.len();

		if len > self.max_debug_window {
			self.debug_window_list.items = self
				.debug_window_list
				.items
				.split_off(len - self.max_debug_window);
		} else {
			self.debug_window_list.state.select(Some(len - 1));
		}
	}
}

pub struct DashVertical {
	active_view: usize,
}

impl DashVertical {
	pub fn new() -> Self {
		DashVertical { active_view: 0 }
	}
}
