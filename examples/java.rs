use jobflow::listener::FlowListener;
use jobflow::{Flow, FlowError, JobError, JobId};
use indicatif::style::ProgressTracker;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use jobflow::job_ordering::GraphTraversalTaskOrderer;

#[path = "../tests/gradle_like.rs"]
mod gradle_like;

struct ProgressLogger {
    start: Instant,
    multi_progress: MultiProgress,
    main: Option<ProgressBar>,
    task_bars: HashMap<JobId, ProgressBar>,
}

impl ProgressLogger {
    pub fn new(m: &MultiProgress) -> ProgressLogger {
        Self {
            start: Instant::now(),
            multi_progress: m.clone(),
            main: None,
            task_bars: HashMap::new(),
        }
    }
}

impl FlowListener for ProgressLogger {
    fn started(&mut self) {
        let style = ProgressStyle::with_template("\n{spinner} {msg} [{elapsed}]").unwrap();
        let main = self.multi_progress.add(ProgressBar::new_spinner());
        main.set_style(style);
        main.enable_steady_tick(Duration::from_millis(100));
        self.main = Some(main);
        self.main.as_mut().unwrap().set_message("Running flow")
    }

    fn task_started(&mut self, id: JobId, nickname: &str) {
        let style = ProgressStyle::with_template("> {msg}").unwrap();
        let bar = self.multi_progress.add(
            ProgressBar::new_spinner()
        );
        bar.set_style(style);
        bar.set_message(nickname.to_owned());
        bar.enable_steady_tick(Duration::from_millis(100));
        self.task_bars.insert(id, bar);
    }

    fn task_finished(&mut self, id: JobId, nickname: &str, _result: Result<(), &JobError>) {
        if let Some(bar) = self.task_bars.remove(&id) {
            bar.finish_and_clear();
            self.multi_progress.println(format!("> {nickname}\n")).unwrap();
        }
    }

    fn finished(&mut self, _result: Result<(), &FlowError>) {
        self.main.as_mut().unwrap().finish_and_clear();
        println!("Finished in {:.3} sec", self.start.elapsed().as_secs_f32());
    }
}

fn main() -> Result<(), FlowError> {
    test_log::tracing_subscriber::fmt().init();
    let mut flow = Flow::builder()
        .with_task_orderer(GraphTraversalTaskOrderer)
        .build();
    gradle_like::create_flow(&mut flow)?;
    let mut m = MultiProgress::new();


    let logger = ProgressLogger::new(&m);
    flow.add_listener(logger);


    flow.run()?;



    Ok(())
}