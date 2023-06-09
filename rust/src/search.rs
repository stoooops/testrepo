use crate::criteria::CriteriaPredicate;
use crate::crypto::AddressGenerator;
use crate::mnemonic_log;
use crate::randnum::NumberGenerator;
use log::info;
use num_format::{Locale, ToFormattedString};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use bip32::{Language, Mnemonic};
use rayon::{current_thread_index, prelude::*, ThreadPool, ThreadPoolBuilder};

pub struct Searcher<'a> {
    number_generator: Box<dyn NumberGenerator + 'a>,
    address_generator: Box<dyn AddressGenerator + 'a>,
    criteria_predicate: Box<dyn CriteriaPredicate + 'a>,
    max_attempts: usize,
}

pub struct SearchResult {
    pub address: String,
    pub seed: [u8; 32],
}

impl<'a> Searcher<'a> {
    pub fn new(
        number_generator: Box<dyn NumberGenerator + 'a>,
        address_generator: Box<dyn AddressGenerator + 'a>,
        criteria_predicate: Box<dyn CriteriaPredicate + 'a>,
        max_attempts: usize,
    ) -> Self {
        Self {
            number_generator,
            address_generator,
            criteria_predicate,
            max_attempts,
        }
    }

    pub fn run(&mut self) -> SearchResult {
        let input_num = self.number_generator.generate();
        let address = self.address_generator.generate(input_num).unwrap();
        let mut best: SearchResult = SearchResult {
            address,
            seed: input_num,
        };
        for _ in 0..self.max_attempts {
            let entropy = self.number_generator.generate();
            let address = self.address_generator.generate(entropy).unwrap();
            if self.criteria_predicate.better(&address, &best.address) {
                best = SearchResult {
                    address,
                    seed: entropy,
                };
            }
        }
        best
    }
}

pub struct ThreadPoolSearcher<'a> {
    thread_pool: ThreadPool,
    num_jobs: usize,
    attempts_per_job: usize,
    number_generator: Box<dyn NumberGenerator + Send + Sync + 'a>,
    address_generator: Box<dyn AddressGenerator + Send + Sync + 'a>,
    criteria_predicate: Box<dyn CriteriaPredicate + Send + Sync + 'a>,
}

impl<'a> ThreadPoolSearcher<'a> {
    pub fn new(
        num_threads: usize,
        num_jobs: usize,
        attempts_per_job: usize,
        number_generator: Box<dyn NumberGenerator + Send + Sync + 'a>,
        address_generator: Box<dyn AddressGenerator + Send + Sync + 'a>,
        criteria_predicate: Box<dyn CriteriaPredicate + Send + Sync + 'a>,
    ) -> Self {
        let thread_pool = ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build()
            .expect("Failed to create thread pool");

        Self {
            thread_pool,
            num_jobs,
            attempts_per_job,
            number_generator,
            address_generator,
            criteria_predicate,
        }
    }

    pub fn run(&self) -> String {
        let best_address = Arc::new(Mutex::new(String::from(
            "0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF",
        )));
        let completed_jobs = Arc::new(AtomicUsize::new(0));

        // logging
        let num_completed_jobs_log_width = format!("{}", self.num_jobs).len();
        let num_threads_log_width = format!("{}", self.thread_pool.current_num_threads()).len();
        let num_searches_log_width = format!("{}", self.num_jobs * self.attempts_per_job).len();

        self.thread_pool.install(|| {
            (0..self.num_jobs)
                .into_par_iter()
                .enumerate()
                .for_each_with(
                    best_address.clone(),
                    |best: &mut Arc<Mutex<String>>, (_job_num, _worker_id)| {

                        // Criteria gets moved here, so we need to clone it
                        // but it's a box so we need to clone the box
                        let mut searcher: Searcher =
                            Searcher::new(
                                self.number_generator.clone_box(),
                                self.address_generator.clone_box(),
                                self.criteria_predicate.clone_box(),
                                self.attempts_per_job);
                        let found: SearchResult = searcher.run();
                        let found_address: &str = found.address.as_str();
                        let num_completed_jobs = completed_jobs.fetch_add(1, Ordering::SeqCst) + 1;
                        let num_completed_searches: usize = num_completed_jobs * self.attempts_per_job;

                        let mut best_address_guard: MutexGuard<String> = best.lock().unwrap();

                        let better: bool = self.criteria_predicate.better(found_address, &*best_address_guard);
                        if better {
                            *best_address_guard = String::from(found_address);
                        }

                        let save = found_address.starts_with("0x00000000");

                        let s: &str = if better { "best" } else if save { "save" } else { "----" };
                        let address: &str = if better || save { found_address } else { best_address_guard.as_str() };

                        if better || save || (num_completed_jobs % 1000) == 0{
                            let thread_index = current_thread_index().unwrap_or(0);
                            info!(
                                "Thread #{:twidth$}     Job #{:jwidth$}     Try #{:swidth$}     {}     {}",
                                thread_index.to_formatted_string(&Locale::en),
                                num_completed_jobs.to_formatted_string(&Locale::en),
                                num_completed_searches.to_formatted_string(&Locale::en),
                                s,
                                address,
                                twidth = num_threads_log_width,
                                jwidth = num_completed_jobs_log_width,
                                swidth = num_searches_log_width
                            );
                        }

                        if save {
                            let mnemonic: Mnemonic = Mnemonic::from_entropy(found.seed, Language::English);
                            mnemonic_log!("{} {}", found_address, mnemonic.phrase());
                        }
                    },
                );
        });

        let best_address_guard: MutexGuard<String> = best_address.lock().unwrap();
        best_address_guard.clone()
    }
}
