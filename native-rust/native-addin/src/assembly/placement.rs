//! Finite absolute translation and separately acknowledged captured position.
use super::*;
pub const SCOPE:&str="Absolute numeric-mm translation of one exact owned ungrounded flat root occurrence with identity orientation. Once-only transform2 followed by one explicitly guarded design-wide captured position; all other poses/definitions unchanged. No rotation, grounding change, pending unrelated positions, rollback, automatic retry or full legacy parity.";
pub const CONTRACT: &str = "native-root-occurrence-translation-v1";
pub const MAX_MM: f64 = 1_000_000.;
pub const MAX_SNAPSHOTS: usize = 64;
pub const EPS_CM: f64 = 1e-8;
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotFact {
    pub slot: usize,
    pub timeline_index: u64,
    pub health: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionState {
    pub pending: bool,
    pub timeline: u64,
    pub timeline_position: u64,
    pub snapshots: Vec<SnapshotFact>,
}
#[derive(Clone, Debug, Default)]
pub struct SnapshotAttempt {
    pub attempt: Attempt,
    pub snapshot_slot: Option<usize>,
}
fn validate_positions(s: &PositionState, p: &Projection) -> Result<()> {
    if s.snapshots.len() > MAX_SNAPSHOTS
        || s.pending != p.root.pending_snapshot
        || s.timeline != p.root.timeline
        || s.timeline_position != p.root.timeline_position
    {
        return Err("native_placement_position_state_mismatch".into());
    }
    let mut indexes = std::collections::BTreeSet::new();
    for (i, f) in s.snapshots.iter().enumerate() {
        if f.slot != i
            || f.timeline_index >= s.timeline
            || f.health != 0
            || !indexes.insert(f.timeline_index)
        {
            return Err("native_placement_snapshot_scope_unsupported".into());
        }
    }
    Ok(())
}
fn capture<B: Backend>(b: &B, h: &B::Handle, pending: bool) -> Result<Projection> {
    if b.command()? != "SelectCommand" {
        return Err("native_placement_idle_command_required".into());
    }
    let a = b.capture(h, false)?;
    if a.projection.root != a.root_after {
        if let Some(d) = root_capture_diagnostic(&a.projection.root, &a.root_after) {
            b.record_observation_diagnostic(d);
        }
        return Err("native_placement_root_changed_during_capture".into());
    }
    validate_projection(&a.projection, pending)?;
    if a.projection.cloud_metadata || a.projection.root.pending_snapshot != pending {
        return Err("native_placement_pending_state_mismatch".into());
    }
    Ok(a.projection)
}
pub(super) fn observe<B: Backend>(
    b: &B,
    h: &B::Handle,
    pending: bool,
) -> Result<(Projection, PositionState)> {
    let positions = b.positions(h)?;
    let a = capture(b, h, pending)?;
    let final_positions = b.positions(h)?;
    // The final root bracket follows every position getter, so a getter-induced
    // revision change cannot escape the complete repeated-capture comparison.
    let z = capture(b, h, pending)?;
    validate_positions(&positions, &a)?;
    validate_positions(&final_positions, &z)?;
    if a != z || positions != final_positions {
        let difference = readback::projection_difference(&a, &z, "placement_strict_capture_pair");
        b.record_observation_diagnostic(json!({"kind":"native_placement_capture_change","firstProjectionDifference":difference,"positionsBefore":positions,"positionsAfter":final_positions,"additionalSdkReads":0,"retryAttempted":false}));
        return Err("native_placement_projection_changed".into());
    }
    Ok((z, final_positions))
}
pub fn matrix(position: &[f64; 3]) -> [f64; 16] {
    [
        1.,
        0.,
        0.,
        position[0] / 10.,
        0.,
        1.,
        0.,
        position[1] / 10.,
        0.,
        0.,
        1.,
        position[2] / 10.,
        0.,
        0.,
        0.,
        1.,
    ]
}
fn exact_occurrence(doc: &str, id: &str, p: &Projection) -> Result<usize> {
    (0..p.occurrences.len())
        .find(|&i| occurrence_id(doc, i) == id)
        .ok_or_else(|| "native_placement_occurrence_not_owned".into())
}
pub(super) fn admit(doc: &str, id: &str, position: &[f64; 3], p: &Projection) -> Result<usize> {
    if position.iter().any(|v| !v.is_finite() || v.abs() > MAX_MM) {
        return Err("native_placement_position_bound".into());
    }
    let i = exact_occurrence(doc, id, p)?;
    if p.occurrences[i].grounded || p.occurrences[i].grounded_to_parent {
        return Err("native_placement_grounded_occurrence".into());
    }
    if p.occurrences[i]
        .matrix
        .iter()
        .zip(matrix(position))
        .all(|(a, b)| (a - b).abs() <= EPS_CM)
    {
        return Err("native_placement_no_change".into());
    }
    Ok(i)
}
fn verify_pose<B: Backend>(
    b: &B,
    before: &Projection,
    after: &Projection,
    oi: usize,
    wanted: &[f64; 16],
    captured: bool,
) -> Result<()> {
    let mut expected = before.clone();
    expected.root.revision = after.root.revision.clone();
    expected.root.pending_snapshot = after.root.pending_snapshot;
    expected.occurrences[oi].matrix = *wanted;
    if captured {
        expected.root.timeline = before.root.timeline + 1;
        expected.root.timeline_position = expected.root.timeline;
    }
    if let Some(d) = readback::projection_difference(&expected, after, "placement_expected_delta") {
        b.record_observation_diagnostic(d);
    }
    if before.components != after.components || before.occurrences.len() != after.occurrences.len()
    {
        return Err("native_placement_unrelated_definition_changed".into());
    }
    let actual = &after.occurrences[oi].matrix;
    if actual
        .iter()
        .zip(wanted)
        .any(|(a, b)| !a.is_finite() || (a - b).abs() > EPS_CM)
    {
        return Err("native_placement_pose_mismatch".into());
    }
    let mut normalized = after.clone();
    normalized.root.revision = before.root.revision.clone();
    normalized.root.pending_snapshot = before.root.pending_snapshot;
    if captured {
        if after.root.timeline != before.root.timeline + 1
            || after.root.timeline_position != after.root.timeline
        {
            return Err("native_placement_timeline_delta".into());
        }
        normalized.root.timeline = before.root.timeline;
        normalized.root.timeline_position = before.root.timeline_position;
    }
    normalized.occurrences[oi].matrix = before.occurrences[oi].matrix;
    if normalized != *before {
        return Err("native_placement_unrelated_state_changed".into());
    }
    Ok(())
}
impl<H> Book<H> {
    pub(super) fn execute_translation<B: Backend<Handle = H>>(
        &mut self,
        b: &B,
        plan: Plan,
    ) -> Value {
        if let Err(e) = self.ready() {
            return error(&e);
        }
        if plan.authority != self.authority {
            return error("native_assembly_foreign_plan");
        }
        let Some(i) = self
            .entries
            .iter()
            .position(|e| e.document == plan.document_id && !e.retired && e.handle.is_some())
        else {
            return error("native_assembly_document_retired");
        };
        let Action::Translate {
            occurrence_id: id,
            position_mm,
        } = &plan.action
        else {
            return error("native_placement_dispatch_mismatch");
        };
        let Some(initial_positions) = plan.placement_before.as_ref() else {
            return error("native_placement_missing_plan");
        };
        let oi = match admit(&plan.document_id, id, position_mm, &plan.before) {
            Ok(i) => i,
            Err(e) => return error(&e),
        };
        let ci = plan.before.occurrences[oi].component;
        let h = self.entries[i].handle.as_ref().unwrap();
        let wanted = matrix(position_mm);
        let check = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
            let (p, s) = observe(b, h, false)?;
            if p != plan.before || s != *initial_positions {
                return Err("target_revision_mismatch".into());
            }
            if s.snapshots.len() >= MAX_SNAPSHOTS || s.timeline >= 4096 {
                return Err("native_placement_snapshot_capacity".into());
            }
            Ok(())
        }));
        match check {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return error(&e),
            Err(_) => return error("native_placement_pre_effect_panic"),
        };
        let mut pose_attempt = Attempt::default();
        let mut snapshot_attempt = SnapshotAttempt::default();
        let mut stage_count = 0;
        let mut intermediate: Option<(Projection, PositionState)> = None;
        let mut observed: Option<(Projection, PositionState)> = None;
        self.faulted = true;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
            pose_attempt.invoked = true;
            pose_attempt.component = Some(ci);
            pose_attempt.occurrence = Some(oi);
            stage_count = 1;
            pose_attempt = b.translate(h, oi, &wanted);
            if !pose_attempt.invoked || !pose_attempt.acknowledged {
                return Err("native_placement_transform_unconfirmed".into());
            }
            if pose_attempt.component != Some(ci) || pose_attempt.occurrence != Some(oi) {
                return Err("native_placement_effect_identity_mismatch".into());
            }
            intermediate = Some(observe(b, h, true)?);
            let (p, s) = intermediate.as_ref().unwrap();
            verify_pose(b, &plan.before, p, oi, &wanted, false)?;
            let mut normalized = s.clone();
            normalized.pending = false;
            if normalized != *initial_positions {
                return Err("native_placement_snapshot_changed_before_capture".into());
            }
            if b.command()? != "SelectCommand" {
                return Err("native_placement_command_changed_before_capture".into());
            }
            snapshot_attempt.attempt.invoked = true;
            stage_count = 2;
            snapshot_attempt = b.capture_position(h);
            if !snapshot_attempt.attempt.invoked || !snapshot_attempt.attempt.acknowledged {
                return Err("native_placement_capture_unconfirmed".into());
            }
            if snapshot_attempt.snapshot_slot != Some(initial_positions.snapshots.len()) {
                return Err("native_placement_snapshot_identity_mismatch".into());
            }
            observed = Some(observe(b, h, false)?);
            let (p, s) = observed.as_ref().unwrap();
            verify_pose(b, &plan.before, p, oi, &wanted, true)?;
            if s.snapshots.len() != initial_positions.snapshots.len() + 1
                || s.snapshots[..initial_positions.snapshots.len()]
                    != initial_positions.snapshots[..]
                || s.snapshots.last().unwrap().timeline_index != initial_positions.timeline
            {
                return Err("native_placement_snapshot_membership_delta".into());
            }
            Ok(())
        }));
        let invoked = pose_attempt.invoked || snapshot_attempt.attempt.invoked;
        let format = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Value> {
            let stages = [
                json!({"stage":"occurrenceTransform","attempt":pose_attempt}),
                json!({"stage":"positionSnapshot","attempt":snapshot_attempt.attempt,"snapshotSlot":snapshot_attempt.snapshot_slot}),
            ];
            let evidence = json!({"coordinateSpace":"root_assembly","inputUnit":"mm","matrixTranslationUnit":"cm","requestedPositionMm":position_mm,"requestedMatrixCm":wanted,"before":initial_positions,"intermediate":intermediate.as_ref().map(|(_,s)|s),"after":observed.as_ref().map(|(_,s)|s),"intermediateProjectionDigest":intermediate.as_ref().and_then(|(p,_)|revision(p).ok()),"observedMatrixCm":observed.as_ref().map(|(p,_)|p.occurrences[oi].matrix),"stableCapturesPerStage":2,"maximumTransformInvocations":1,"maximumCaptureInvocations":1,"effectRetryAttempted":false});
            let failed = match &result {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.as_str()),
                Err(_) => Some("native_placement_effect_or_readback_panic"),
            };
            if let Some(e) = failed {
                return Ok(
                    json!({"success":false,"errorCode":e,"nativeExecutionStarted":invoked,"stages":&stages[..stage_count],"placementEvidence":evidence,"sdkDiagnostic":diagnostic(b),"automaticReplayAllowed":false,"rollbackAttempted":false}),
                );
            }
            let data = output(&plan.document_id, &observed.as_ref().unwrap().0)?;
            let reference = json!({"documentId":plan.document_id,"revision":data["target"]["revision"],"objectType":"Occurrence","occurrenceId":id});
            Ok(
                json!({"success":true,"data":data,"occurrenceId":id,"occurrenceRef":reference,"stages":&stages[..stage_count],"placementEvidence":evidence,"nativeExecutionStarted":true,"automaticReplayAllowed":false,"rollbackAttempted":false,"legacyParity":false}),
            )
        }));
        let mut response = match format {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                json!({"success":false,"errorCode":e,"nativeExecutionStarted":invoked,"poseAttempt":pose_attempt,"snapshotAttempt":snapshot_attempt.attempt})
            }
            Err(_) => {
                json!({"success":false,"errorCode":"native_placement_observation_failed","nativeExecutionStarted":invoked,"poseAttempt":pose_attempt,"snapshotAttempt":snapshot_attempt.attempt})
            }
        };
        if response["success"] == true || !invoked {
            self.faulted = false;
        } else {
            response["outcome"] = json!("unknown");
            response["reconciliationRequired"] = json!(true);
        }
        response
    }
}
