//! Exact document-state classification; this module grants no filesystem authority.
use crate::{
    skill_dotagents_ledger::DotagentsDetachIntent, skill_fork_registry::ForkRegistry,
    skill_fork_repair_intent::DotagentsForkRepairIntent,
};

pub struct NativeForkDocuments<'a> {
    pub manifest: &'a [u8],
    pub lock: &'a [u8],
    /// None means proven absence, never an unreadable or failed registry read.
    pub registry: Option<&'a [u8]>,
    pub skill: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeForkProgress {
    Prepared,
    ManifestPublished,
    ProviderDetached,
    RegistryPublished,
    RepairPublished,
}

fn bounded(documents: &NativeForkDocuments<'_>) -> Result<(), String> {
    if documents.manifest.len() > 8 * 1024 * 1024
        || documents.lock.len() > 8 * 1024 * 1024
        || documents
            .registry
            .is_some_and(|bytes| bytes.len() > 8 * 1024 * 1024)
        || documents.skill.len() > crate::skill_service::MAX_REPAIR_DOCUMENT_BYTES
    {
        return Err("Native fork document exceeds its limit".into());
    }
    Ok(())
}

pub fn classify_native_fork(
    intent: &DotagentsForkRepairIntent,
    original: &NativeForkDocuments<'_>,
    current: &NativeForkDocuments<'_>,
) -> Result<NativeForkProgress, String> {
    bounded(original)?;
    bounded(current)?;
    intent.validate_for_operation(intent.snapshots().operation_id())?;
    let proposal = intent
        .provider_documents()
        .ok_or("Legacy fork has no native proposal")?;
    let lock = std::str::from_utf8(original.lock).map_err(|error| error.to_string())?;
    let manifest = std::str::from_utf8(original.manifest).map_err(|error| error.to_string())?;
    intent.validate_saved_provider_documents(lock, manifest)?;
    let detach = DotagentsDetachIntent::from_documents(&intent.repair().name, lock, manifest)?;
    let source = detach.fork_source()?;
    let record = intent.registry().record();
    if record.origin_source != source.source()
        || record.repo != source.repo()
        || record.path != source.path()
        || record.base_commit != source.commit()
        || record.declared_ref.as_deref() != source.declared_ref()
    {
        return Err("Native fork originals differ from the recorded source".into());
    }
    intent.repair().validate_original(original.skill)?;
    let registry_before = original.registry.unwrap_or(b"{}");
    let registry: ForkRegistry =
        serde_json::from_slice(registry_before).map_err(|error| error.to_string())?;
    if serde_json::to_value(&registry).map_err(|error| error.to_string())?
        != serde_json::to_value(
            intent
                .repair()
                .fork_registry_before
                .as_ref()
                .ok_or("Missing original registry projection")?,
        )
        .map_err(|error| error.to_string())?
    {
        return Err("Native fork original registry differs from intent".into());
    }
    let registry_after = intent.registry().apply_document(registry_before)?;
    let before = [
        Some(original.manifest),
        Some(original.lock),
        original.registry,
        Some(original.skill),
    ];
    let after = [
        Some(proposal.manifest().as_bytes()),
        Some(proposal.lock().as_bytes()),
        Some(registry_after.as_slice()),
        Some(intent.repair().proposed_content.as_bytes()),
    ];
    let observed = [
        Some(current.manifest),
        Some(current.lock),
        current.registry,
        Some(current.skill),
    ];
    let stages = [
        NativeForkProgress::Prepared,
        NativeForkProgress::ManifestPublished,
        NativeForkProgress::ProviderDetached,
        NativeForkProgress::RegistryPublished,
        NativeForkProgress::RepairPublished,
    ];
    for (prefix, stage) in stages.into_iter().enumerate() {
        if observed.iter().enumerate().all(|(index, value)| {
            *value
                == if index < prefix {
                    after[index]
                } else {
                    before[index]
                }
        }) {
            return Ok(stage);
        }
    }
    Err("Native fork files do not match a valid publication prefix".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_exact_prefixes_for_present_and_absent_registries() {
        for registry_present in [false, true] {
            let lock = format!("version = 1\n[skills.alpha]\nsource = 'owner/repo'\nresolved_path = 'skills/alpha'\nresolved_commit = '{}'\n", "a".repeat(40));
            let manifest = "version = 1\n[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\n";
            let skill = "---\nname: alpha\ndescription: Use when: testing\n---\nbody\n";
            let proposal = DotagentsDetachIntent::from_documents("alpha", &lock, manifest)
                .unwrap()
                .propose_document_detach(&lock, manifest)
                .unwrap();
            let mut value =
                serde_json::to_value(crate::skill_fork_repair_intent::tests::fixture()).unwrap();
            value["version"] = serde_json::json!(2);
            value["provider_documents"] = serde_json::to_value(&proposal).unwrap();
            let intent: DotagentsForkRepairIntent = serde_json::from_value(value).unwrap();
            let original = NativeForkDocuments {
                manifest: manifest.as_bytes(),
                lock: lock.as_bytes(),
                registry: registry_present.then_some(b"{}".as_slice()),
                skill: skill.as_bytes(),
            };
            let registry_after = intent.registry().apply_document(b"{}").unwrap();
            let before = [
                Some(original.manifest),
                Some(original.lock),
                original.registry,
                Some(original.skill),
            ];
            let after = [
                Some(proposal.manifest().as_bytes()),
                Some(proposal.lock().as_bytes()),
                Some(registry_after.as_slice()),
                Some(intent.repair().proposed_content.as_bytes()),
            ];
            for mask in 0..16 {
                let bytes: Vec<_> = (0..4)
                    .map(|index| {
                        if mask & (1 << index) == 0 {
                            before[index]
                        } else {
                            after[index]
                        }
                    })
                    .collect();
                let current = NativeForkDocuments {
                    manifest: bytes[0].unwrap(),
                    lock: bytes[1].unwrap(),
                    registry: bytes[2],
                    skill: bytes[3].unwrap(),
                };
                let result = classify_native_fork(&intent, &original, &current);
                let expected = match mask {
                    0 => Some(NativeForkProgress::Prepared),
                    1 => Some(NativeForkProgress::ManifestPublished),
                    3 => Some(NativeForkProgress::ProviderDetached),
                    7 => Some(NativeForkProgress::RegistryPublished),
                    15 => Some(NativeForkProgress::RepairPublished),
                    _ => None,
                };
                assert_eq!(
                    result.ok(),
                    expected,
                    "mask={mask}, registry={registry_present}"
                );
            }
            let changed = NativeForkDocuments {
                manifest: b"# unrelated edit\n",
                ..original
            };
            assert!(classify_native_fork(&intent, &original, &changed).is_err());
            let wrong_source = lock.replace(&"a".repeat(40), &"b".repeat(40));
            let wrong_original = NativeForkDocuments {
                lock: wrong_source.as_bytes(),
                ..original
            };
            assert!(classify_native_fork(&intent, &wrong_original, &wrong_original).is_err());
        }
    }
}
