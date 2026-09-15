pub(crate) use skill_studio_core::skill_document_write::{
    begin_skill_md_write_transaction, write_skill_md, write_skill_md_compare_and_swap,
    SkillMdWriteTransaction,
};

#[cfg(test)]
pub(crate) use skill_studio_core::skill_document_write::{
    skill_md_write_transaction_is_held, write_skill_md_bytes, write_skill_md_compare_and_swap_with,
};

#[cfg(test)]
mod tests {
    #[test]
    fn desktop_and_history_share_the_document_transaction() {
        let _transaction =
            skill_studio_core::skill_document_write::begin_skill_md_write_transaction().unwrap();
        assert!(super::skill_md_write_transaction_is_held());
    }
}
