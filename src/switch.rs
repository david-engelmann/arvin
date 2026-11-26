//! Switch state file lock, atomic publication, and public operations.

use std::cmp::Ordering;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use fs2::FileExt;

use crate::evaluate::{
    consultation_fields, missed_proofs, reference_instant, stage_index, warrant,
    ConsultationFields, ModelTransport, Seat, StageReport,
};
use crate::instant::{format_instant, validate_now, Instant};
use crate::join::{
    empty_document, missed_warrant_index, model_name_shape_ok, validate_document, Document, Record,
    Run,
};
use crate::reader::{parse_document, ParseFailure};
use crate::transport::hex_text;
use crate::validate::{
    run_id_text, validate_ceiling, validate_missed_proof_limit, validate_name, validate_note,
    validate_proof_interval, validate_spend_amount,
};
use crate::value::{corrupt, py_repr_str, Error, JsonInt, Result, Value};
use crate::vignette::render_vignette;
use crate::writer::write_document;
use crate::{
    ANCESTRY_BELOW_BAR, DE_ESCALATION_BAR, ESCALATION_BAR, HOST_CONTEXT_MAX, LOCK_SUFFIX,
    SPEND_MAX, STAGES,
};

struct LockGuard {
    file: File,
    lock_path: PathBuf,
}

impl LockGuard {
    /// Whether the file this guard holds is the file standing
    /// at the lock path now.
    fn identity_holds(&self) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let held = match self.file.metadata() {
                Ok(metadata) => metadata,
                Err(_) => return false,
            };
            let standing = match std::fs::metadata(&self.lock_path) {
                Ok(metadata) => metadata,
                Err(_) => return false,
            };
            held.dev() == standing.dev() && held.ino() == standing.ino()
        }
        #[cfg(not(unix))]
        {
            // On Windows, Switch::locked opens the lock file without
            // FILE_SHARE_DELETE so the operating system prevents
            // unlinking or renaming while the handle stands open.
            let _ = (&self.file, &self.lock_path);
            true
        }
    }

    /// Refuse to go on when the held lock is not the file at
    /// the lock path, because anything this command published
    /// could then be overwritten by the command that locked the
    /// standing file, and a success of this command could be
    /// erased without a trace.
    fn verify(&self) -> Result<()> {
        if self.identity_holds() {
            Ok(())
        } else {
            Err(Error::Failure(format!(
                "the lock file at {} is not the file this command locked, \
                 so the state was left unchanged",
                self.lock_path.display()
            )))
        }
    }
}

fn has_non_dir_ancestor(path: &std::path::Path) -> bool {
    let mut ancestor = path.parent();
    while let Some(dir) = ancestor {
        if dir.as_os_str().is_empty() {
            break;
        }
        if let Ok(meta) = std::fs::metadata(dir) {
            return !meta.is_dir();
        }
        ancestor = dir.parent();
    }
    false
}

/// One staged cutoff in one state file, opened fresh on every
/// call.
pub struct Switch {
    path: PathBuf,
    lock_path: PathBuf,
}

impl Switch {
    pub fn new(path: PathBuf) -> Switch {
        let lock_path = match path.file_name() {
            Some(name) => {
                let mut lock_name = name.to_os_string();
                lock_name.push(LOCK_SUFFIX);
                path.with_file_name(lock_name)
            }
            None => {
                let mut raw = path.clone().into_os_string();
                raw.push(LOCK_SUFFIX);
                PathBuf::from(raw)
            }
        };
        Switch { path, lock_path }
    }

    fn locked(&self, exclusive: bool) -> Result<LockGuard> {
        // The identity of the locked file is checked as soon as
        // the lock stands. A mismatch means the file at the path
        // was deleted and replaced under this command, so the
        // command locks the standing file on a bounded retry
        // instead of working under a file no other command can
        // see, and refuses when the path will not settle.
        for attempt in 1..=5 {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o644);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(0x00000001 | 0x00000002);
            }
            let file = options.open(&self.lock_path).map_err(|err| {
                Error::Failure(format!(
                    "the lock file beside the state file cannot be opened, {err}"
                ))
            })?;
            let result = if exclusive {
                file.lock_exclusive()
            } else {
                file.lock_shared()
            };
            result.map_err(|err| {
                Error::Failure(format!(
                    "the lock file beside the state file cannot be locked, {err}"
                ))
            })?;
            let guard = LockGuard {
                file,
                lock_path: self.lock_path.clone(),
            };
            if guard.identity_holds() {
                return Ok(guard);
            }
            if attempt == 5 {
                return Err(Error::Failure(format!(
                    "the lock file at {} is not the file this command locked, \
                     so the state was left unchanged",
                    self.lock_path.display()
                )));
            }
        }
        unreachable!()
    }

    /// Read and strictly validate the state, or start empty.
    ///
    /// A missing file reads as the empty document. A file that
    /// stands but cannot be read as recorded raises the corrupt
    /// error and is never reset and never written over by a read.
    fn read_document(&self) -> Result<Document> {
        let raw = match std::fs::read(&self.path) {
            Ok(raw) => raw,
            Err(err)
                if err.kind() == std::io::ErrorKind::NotFound
                    && !has_non_dir_ancestor(&self.path) =>
            {
                return Ok(empty_document());
            }
            Err(err) => return Err(corrupt(format!("the state file cannot be read, {err}"))),
        };
        let value = parse_document(&raw).map_err(|failure| match failure {
            ParseFailure::DuplicateKey(key) => corrupt(format!(
                "a key stands twice in the document, {}",
                py_repr_str(&key)
            )),
            ParseFailure::Syntax(detail) => {
                corrupt(format!("the document does not parse, {detail}"))
            }
        })?;
        validate_document(&value)
    }

    fn publish(&self, document: &Document, guard: &LockGuard) -> Result<()> {
        guard.verify()?;
        let text = write_document(document);
        let mut name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        name = format!(".{name}.tmp-{}-", std::process::id());
        let mut draw = [0u8; 4];
        getrandom::fill(&mut draw).map_err(|err| {
            Error::Failure(format!("the publication draw could not be made, {err}"))
        })?;
        name.push_str(&hex_text(&draw));
        let temp = self.path.with_file_name(name);
        // The temporary file is opened exclusively, so a plant
        // standing at the name fails the publish instead of being
        // followed and written through, the deliberate divergence
        // the port amendment records.
        let written = (|| -> std::io::Result<()> {
            let mut handle = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            handle.write_all(text.as_bytes())?;
            handle.sync_all()?;
            Ok(())
        })();
        if let Err(err) = written {
            let _ = std::fs::remove_file(&temp);
            return Err(Error::Failure(format!(
                "the state file cannot be published, {err}"
            )));
        }
        // The held lock's identity is verified once more with the
        // rename as the very next step, so the window between the
        // check and the publication stands as narrow as the
        // design can make it. A failure here leaves the state
        // unchanged under the identity error's own message.
        if let Err(err) = guard.verify() {
            let _ = std::fs::remove_file(&temp);
            return Err(err);
        }
        if let Err(err) = std::fs::rename(&temp, &self.path) {
            let _ = std::fs::remove_file(&temp);
            return Err(Error::Failure(format!(
                "the state file cannot be published, {err}"
            )));
        }
        // A best effort fsync of the holding directory, as the
        // Python publication performs it.
        let parent = self.path.parent().map(|parent| {
            if parent.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                parent.to_path_buf()
            }
        });
        if let Some(parent) = parent {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    }

    fn ensure_parent(&self) -> Result<()> {
        let parent = match self.path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => return Ok(()),
        };
        std::fs::create_dir_all(&parent).map_err(|err| {
            Error::Failure(format!(
                "the directory holding the state file cannot be created, {err}"
            ))
        })
    }

    fn mutate<T>(
        &self,
        now: &Instant,
        action: impl FnOnce(&mut Document) -> Result<T>,
    ) -> Result<T> {
        validate_now(now)?;
        self.ensure_parent()?;
        let guard = self.locked(true)?;
        let mut document = self.read_document()?;
        let result = action(&mut document)?;
        self.publish(&document, &guard)?;
        Ok(result)
    }

    fn read<T>(&self, action: impl FnOnce(&Document) -> Result<T>) -> Result<T> {
        // Only an absent file reads as the empty document, the
        // discrimination read_document makes on the read itself.
        // A path that cannot be examined at all stands as a
        // state file that cannot be read, the corrupt error
        // DM-8 seats, and never as an absent file.
        let absent = match std::fs::metadata(&self.path) {
            Ok(_) => false,
            Err(err)
                if err.kind() == std::io::ErrorKind::NotFound
                    && !has_non_dir_ancestor(&self.path) =>
            {
                true
            }
            Err(err) => return Err(corrupt(format!("the state file cannot be read, {err}"))),
        };
        if absent {
            return action(&empty_document());
        }
        let _guard = self.locked(false)?;
        action(&self.read_document()?)
    }

    fn append_record(
        run: &mut Run,
        kind: &str,
        now: &Instant,
        stage: &str,
        reasons: &[&str],
    ) -> Result<()> {
        Self::append_record_with_ancestry(run, kind, now, stage, reasons, None)
    }

    /// Append a plain record, carrying the ancestry pattern
    /// of DM-22 where the step down it writes stands certified
    /// under DM-23, and the null pattern everywhere else.
    fn append_record_with_ancestry(
        run: &mut Run,
        kind: &str,
        now: &Instant,
        stage: &str,
        reasons: &[&str],
        model_ancestry: Option<String>,
    ) -> Result<()> {
        let at = format_instant(now)?;
        run.log.push(Record {
            seq: run.log.len() as u64 + 1,
            kind: kind.to_string(),
            at,
            instant: *now,
            stage: stage.to_string(),
            reasons: reasons.iter().map(|reason| reason.to_string()).collect(),
            model_name: None,
            model_verdict: None,
            model_confidence: None,
            model_score: None,
            fallback_taken: None,
            model_ancestry,
        });
        Ok(())
    }

    /// Append the stage record that carries a consultation, the
    /// one act DM-13 makes of the consultation and its record.
    fn append_consultation_record(
        run: &mut Run,
        now: &Instant,
        stage: &str,
        reasons: &[&str],
        fields: ConsultationFields,
    ) -> Result<()> {
        let at = format_instant(now)?;
        run.log.push(Record {
            seq: run.log.len() as u64 + 1,
            kind: "stage".to_string(),
            at,
            instant: *now,
            stage: stage.to_string(),
            reasons: reasons.iter().map(|reason| reason.to_string()).collect(),
            model_name: fields.model_name,
            model_verdict: fields.model_verdict,
            model_confidence: fields.model_confidence,
            model_score: fields.model_score,
            fallback_taken: fields.fallback_taken,
            model_ancestry: None,
        });
        Ok(())
    }

    fn require_run(document: &Document, run_id: &str) -> Result<usize> {
        match document.runs.iter().position(|run| run.id == run_id) {
            Some(index) => Ok(index),
            None => Err(Error::NotFound(format!(
                "run {} does not stand in the state file",
                py_repr_str(run_id)
            ))),
        }
    }

    /// Open a run record and record the registration.
    pub fn register(
        &self,
        run_id: &Value,
        now: &Instant,
        proof_interval_seconds: &Value,
        missed_proof_limit: &Value,
        soft_ceiling: &Value,
        hard_ceiling: &Value,
    ) -> Result<()> {
        validate_name(run_id)?;
        let proof_interval_seconds = validate_proof_interval(proof_interval_seconds)?;
        let missed_proof_limit = validate_missed_proof_limit(missed_proof_limit)?;
        let soft_ceiling = validate_ceiling(soft_ceiling, "soft ceiling")?;
        let hard_ceiling = validate_ceiling(hard_ceiling, "hard ceiling")?;
        if let (Some(soft), Some(hard)) = (soft_ceiling, hard_ceiling) {
            if soft > hard {
                return Err(Error::InvalidRequest(
                    "the soft ceiling stands above the hard ceiling, declare the soft \
                     ceiling at or below the hard ceiling"
                        .to_string(),
                ));
            }
        }
        let run_id = run_id_text(run_id);
        self.mutate(now, |document| {
            if document.runs.iter().any(|run| run.id == run_id) {
                return Err(Error::Refusal(format!(
                    "run {} already stands in the state file",
                    py_repr_str(&run_id)
                )));
            }
            let mut run = Run {
                id: run_id.clone(),
                registered_at: format_instant(now)?,
                registered_instant: *now,
                proof_interval_seconds: JsonInt::Small(proof_interval_seconds as i128),
                missed_proof_limit,
                soft_ceiling,
                hard_ceiling,
                spend_total: 0,
                last_proof_at: None,
                last_proof_instant: None,
                stage: "none".to_string(),
                clear_streak: JsonInt::Small(0),
                killed: false,
                streak_ancestry: false,
                log: Vec::new(),
            };
            Self::append_record(&mut run, "registered", now, "none", &[])?;
            document.runs.push(run);
            Ok(())
        })
    }

    /// Record a progress proof. Content is never judged in this
    /// slice.
    pub fn prove(&self, run_id: &Value, note: &Value, now: &Instant) -> Result<()> {
        validate_name(run_id)?;
        validate_note(note)?;
        let run_id = run_id_text(run_id);
        self.mutate(now, |document| {
            let index = Self::require_run(document, &run_id)?;
            let run = &mut document.runs[index];
            if !run.killed {
                let stands = match run.last_proof_instant {
                    None => true,
                    Some(standing) => *now > standing,
                };
                if stands {
                    run.last_proof_at = Some(format_instant(now)?);
                    run.last_proof_instant = Some(*now);
                }
            }
            let stage = run.stage.clone();
            Self::append_record(run, "proof", now, &stage, &[])
        })
    }

    /// Record a spend report and return the accumulated spend.
    pub fn spend(&self, run_id: &Value, amount: &Value, now: &Instant) -> Result<i64> {
        validate_name(run_id)?;
        let amount = validate_spend_amount(amount)?;
        let run_id = run_id_text(run_id);
        self.mutate(now, |document| {
            let index = Self::require_run(document, &run_id)?;
            let run = &mut document.runs[index];
            if !run.killed {
                if run.spend_total + amount > SPEND_MAX {
                    return Err(Error::InvalidRequest(format!(
                        "the spend report of {amount} would carry the accumulated \
                         spend of run {} past {SPEND_MAX}",
                        py_repr_str(&run_id)
                    )));
                }
                run.spend_total += amount;
            }
            let stage = run.stage.clone();
            Self::append_record(run, "spend", now, &stage, &[])?;
            Ok(run.spend_total)
        })
    }

    /// Pull the switch. A pull goes straight to kill and latches
    /// it.
    pub fn pull(&self, run_id: &Value, now: &Instant) -> Result<String> {
        validate_name(run_id)?;
        validate_now(now)?;
        let run_id = run_id_text(run_id);
        self.ensure_parent()?;
        let guard = self.locked(true)?;
        let mut document = self.read_document()?;
        let index = Self::require_run(&document, &run_id)?;
        if document.runs[index].killed {
            return Ok("kill".to_string());
        }
        let run = &mut document.runs[index];
        run.stage = "kill".to_string();
        run.killed = true;
        run.clear_streak = JsonInt::Small(0);
        // A pull ends the run, and the certification of DM-23
        // belongs to the stage history it closes, so it clears
        // with the streak.
        run.streak_ancestry = false;
        Self::append_record(run, "pull", now, "kill", &["operator pull"])?;
        self.publish(&document, &guard)?;
        Ok("kill".to_string())
    }

    /// Evaluate one run at the instant and report its stage and
    /// reasons and the record the evaluation appended.
    pub fn stage(&self, run_id: &Value, now: &Instant) -> Result<StageReport> {
        self.evaluate(run_id, now, None)
    }

    /// Evaluate one run at the instant with the model entry
    /// point of DM-13, consulting the caller supplied transport
    /// at most once where a seat of DM-13 stands open, and
    /// report the stage and reasons the combined evaluation
    /// reaches and the record the evaluation appended. With
    /// no transport configured the evaluation is
    /// the stage operation exactly. A host context supplied
    /// with no transport to carry it, and a host context past
    /// the bound of DM-14, are caller errors refused before any
    /// state changes.
    pub fn stage_with_model(
        &self,
        run_id: &Value,
        now: &Instant,
        transport: Option<&dyn ModelTransport>,
        host_context: Option<&str>,
    ) -> Result<StageReport> {
        if host_context.is_some() && transport.is_none() {
            return Err(Error::InvalidRequest(
                "a host context stands supplied with no model transport to carry it".to_string(),
            ));
        }
        if let Some(context) = host_context {
            if context.chars().count() > HOST_CONTEXT_MAX {
                return Err(Error::InvalidRequest(format!(
                    "the host context stands past the bound of {HOST_CONTEXT_MAX} \
                     characters"
                )));
            }
        }
        self.evaluate(
            run_id,
            now,
            transport.map(|carrier| (carrier, host_context)),
        )
    }

    /// The shared evaluation path of the two entry points, so
    /// the stale evaluation rule of DM-19 stands in both. The
    /// deterministic branches are the landed evaluation, and a
    /// consultation stands inside the same exclusive lock,
    /// decided from the state the lock protects.
    fn evaluate(
        &self,
        run_id: &Value,
        now: &Instant,
        model: Option<(&dyn ModelTransport, Option<&str>)>,
    ) -> Result<StageReport> {
        validate_name(run_id)?;
        validate_now(now)?;
        let run_id = run_id_text(run_id);
        self.ensure_parent()?;
        let guard = self.locked(true)?;
        let mut document = self.read_document()?;
        let index = Self::require_run(&document, &run_id)?;
        if document.runs[index].killed {
            let (_, reasons) = warrant(&document.runs[index], now)?;
            return Ok(StageReport {
                run: run_id,
                stage: "kill".to_string(),
                reasons: reasons.iter().map(|reason| reason.to_string()).collect(),
                appended: None,
            });
        }
        // The length of the log before the branches below
        // run. An evaluation appends at most one record, so
        // the report at the tail reads the appended record
        // off the log's growth across the evaluation, inside
        // the one lock, and no caller reads the log back to
        // find it.
        let log_len = document.runs[index].log.len();
        let (warranted, reasons) = warrant(&document.runs[index], now)?;
        let recorded = stage_index(&document.runs[index].stage);
        let stale = *now < reference_instant(&document.runs[index]);
        // The seats of DM-13. The held seat stands where the
        // warrant stands equal to a middle recorded stage. The
        // fall seat stands where the warrant stands below a
        // middle recorded stage with the standing clear streak
        // at 1 and the instant fresh, because the fall decision
        // seat stands closed to a stale evaluation under DM-19.
        // No other evaluation consults the model.
        let streak_at_one = document.runs[index].clear_streak.cmp_i128(1) == Ordering::Equal;
        let middle = matches!(recorded, 1..=3);
        let seat = if model.is_some() && middle && warranted == recorded {
            Some(Seat::Held)
        } else if model.is_some() && middle && warranted < recorded && streak_at_one && !stale {
            Some(Seat::Fall)
        } else {
            None
        };
        let mut changed = false;
        if let Some(seat) = seat {
            let (transport, context) =
                model.expect("a seat stands only where a transport stands configured");
            let pinned = transport.pinned_model_name();
            if !model_name_shape_ok(pinned) {
                return Err(Error::InvalidRequest(format!(
                    "the model transport stands pinned to a model name outside \
                     the shape a record can carry, {}",
                    py_repr_str(pinned)
                )));
            }
            let (missed, vignette) = {
                let run = &document.runs[index];
                let missed = missed_proofs(run, now)?;
                let vignette =
                    render_vignette(run, now, warranted, &reasons, missed, &seat, context);
                (missed, vignette)
            };
            let fields = consultation_fields(transport.consult(&vignette), pinned);
            match seat {
                Seat::Held => {
                    // The combination rule at the held seat. An
                    // escalate verdict at or above the bar rises
                    // the stage by exactly one step, and every
                    // other outcome holds it, and the record
                    // carries the consultation either way.
                    let applies = fields.model_verdict.as_deref() == Some("escalate")
                        && fields
                            .model_confidence
                            .is_some_and(|confidence| confidence >= ESCALATION_BAR);
                    let run = &mut document.runs[index];
                    // A held seat consultation is no certifying
                    // act under DM-23, at any verdict and any
                    // confidence and any freshness, so it clears
                    // the standing certification as it appends
                    // its record, because a consultation that
                    // intervenes between the certifying spend
                    // and the step down ends the ancestry.
                    run.streak_ancestry = false;
                    if applies {
                        run.stage = STAGES[recorded + 1].to_string();
                    }
                    if applies || !stale {
                        run.clear_streak = JsonInt::Small(0);
                    }
                    let stage = run.stage.clone();
                    Self::append_consultation_record(run, now, &stage, &reasons, fields)?;
                }
                Seat::Fall => {
                    // The combination rule at the fall seat. The
                    // step down is endorsed only where the
                    // verdict is de-escalate at or above the
                    // high bar and the cleared signals predicates
                    // of DM-15 hold, the missed proof warrant
                    // standing below the recorded stage and the
                    // spend standing below each standing ceiling,
                    // the warrant standing below the stage being
                    // the seat's own term. Every other outcome
                    // holds the stage and spends the streak, and
                    // the record carries the consultation.
                    let endorsed = {
                        let run = &document.runs[index];
                        fields.model_verdict.as_deref() == Some("de-escalate")
                            && fields
                                .model_confidence
                                .is_some_and(|confidence| confidence >= DE_ESCALATION_BAR)
                            && missed_warrant_index(missed, run.missed_proof_limit) < recorded
                            && run
                                .soft_ceiling
                                .is_none_or(|ceiling| run.spend_total < ceiling)
                            && run
                                .hard_ceiling
                                .is_none_or(|ceiling| run.spend_total < ceiling)
                    };
                    // The certification of DM-23 is decided
                    // here, at the fall seat act itself, and
                    // nowhere else. Where the consultation
                    // answers de-escalate with confidence below
                    // the de-escalation bar, the writer sets
                    // the certification in the same act that
                    // appends the consultation record and
                    // spends the streak. Every other fall seat
                    // outcome clears it, an endorsed step down,
                    // a fallback, an abstention, and an escalate
                    // verdict among them, because none of those
                    // is the ancestry the pattern names.
                    let certifies = fields.model_verdict.as_deref() == Some("de-escalate")
                        && fields
                            .model_confidence
                            .is_some_and(|confidence| confidence < DE_ESCALATION_BAR);
                    let run = &mut document.runs[index];
                    if endorsed {
                        run.stage = STAGES[recorded - 1].to_string();
                    }
                    run.clear_streak = JsonInt::Small(0);
                    run.streak_ancestry = certifies;
                    let stage = run.stage.clone();
                    Self::append_consultation_record(run, now, &stage, &reasons, fields)?;
                }
            }
            changed = true;
        } else if warranted > recorded {
            let run = &mut document.runs[index];
            run.stage = STAGES[warranted].to_string();
            run.clear_streak = JsonInt::Small(0);
            // A rise appends a stage record and resets the
            // streak, so it clears any standing certification
            // under DM-23 as it does so.
            run.streak_ancestry = false;
            if run.stage == "kill" {
                run.killed = true;
            }
            let stage = run.stage.clone();
            Self::append_record(run, "stage", now, &stage, &reasons)?;
            changed = true;
        } else if warranted == recorded {
            let run = &mut document.runs[index];
            if !stale && !run.clear_streak.is_zero() {
                run.clear_streak = JsonInt::Small(0);
                // A streak reset outside the fall seat clears
                // the certification with the streak under
                // DM-23.
                run.streak_ancestry = false;
                changed = true;
            }
            if recorded != stage_index("none") {
                // A hold appended on this branch is a stage
                // record, so it clears any standing
                // certification under DM-23.
                run.streak_ancestry = false;
                let stage = run.stage.clone();
                Self::append_record(run, "stage", now, &stage, &reasons)?;
                changed = true;
            }
        } else if !stale {
            let run = &mut document.runs[index];
            run.clear_streak = run.clear_streak.add_one();
            changed = true;
            if run.clear_streak.cmp_i128(2) != Ordering::Less {
                run.stage = STAGES[recorded - 1].to_string();
                run.clear_streak = JsonInt::Small(0);
                // The step down this branch writes consumes
                // any standing certification of DM-23. Where
                // the certification stands, the record carries
                // the ancestry pattern. Where it does not, the
                // record carries the null pattern exactly as
                // before the amendment. Either way the act
                // clears the field as it resets the streak, so
                // one certification certifies exactly one step
                // down.
                let ancestry = if run.streak_ancestry {
                    Some(ANCESTRY_BELOW_BAR.to_string())
                } else {
                    None
                };
                run.streak_ancestry = false;
                let stage = run.stage.clone();
                Self::append_record_with_ancestry(run, "stage", now, &stage, &reasons, ancestry)?;
            }
        }
        if changed {
            self.publish(&document, &guard)?;
        }
        Ok(StageReport {
            run: run_id,
            stage: document.runs[index].stage.clone(),
            reasons: reasons.iter().map(|reason| reason.to_string()).collect(),
            appended: if document.runs[index].log.len() > log_len {
                document.runs[index].log.last().cloned()
            } else {
                None
            },
        })
    }

    /// Return the recorded decision log of one run, in order.
    pub fn log(&self, run_id: &Value) -> Result<Vec<Record>> {
        validate_name(run_id)?;
        let run_id = run_id_text(run_id);
        self.read(|document| {
            let index = Self::require_run(document, &run_id)?;
            Ok(document.runs[index].log.clone())
        })
    }
}
