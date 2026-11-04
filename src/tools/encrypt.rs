//! Tool 10: AES-256 encryption and decryption.

use lopdf::Permissions;
use medpdf::{encrypt_document, EncryptionAlgorithm, EncryptionParams};

use crate::engine::{pdf_io, validate};
use crate::model::{ToolError, ToolId, ToolOutcome};
use crate::tools::{
    check_cancel,
    split::{ensure_pdf, single_input},
    CancelFlag, Progress, Tool, ToolOptions, ToolRequest,
};

pub struct Encrypt;

impl Tool for Encrypt {
    fn id(&self) -> ToolId {
        ToolId::Encrypt
    }

    fn validate(&self, req: &ToolRequest) -> Result<(), ToolError> {
        let path = single_input(req)?;
        ensure_pdf(path)?;
        // An empty owner password is not an error: the panel says it may be
        // left blank, and `execute` then reuses the user password. Requiring it
        // here made the panel promise something the tool refused.
        if let ToolOptions::Encrypt {
            encrypt,
            user_password,
            ..
        } = &req.options
        {
            if *encrypt && user_password.is_empty() {
                return Err(ToolError::UnsupportedFormat {
                    path: None,
                    detail: "加密需要设置用户密码".to_string(),
                });
            }
            // Decryption is validated in `execute`, after the file has been
            // checked for encryption: asking for a password up front made an
            // unencrypted file impossible to process at all.
        }
        Ok(())
    }

    fn execute(
        &self,
        req: &ToolRequest,
        progress: &Progress,
        cancel: &CancelFlag,
    ) -> Result<ToolOutcome, ToolError> {
        let path = single_input(req)?;
        let (encrypt, user_password, owner_password, allow_print, allow_copy, allow_modify) =
            match &req.options {
                ToolOptions::Encrypt {
                    encrypt,
                    user_password,
                    owner_password,
                    allow_print,
                    allow_copy,
                    allow_modify,
                } => (
                    *encrypt,
                    user_password.clone(),
                    owner_password.clone(),
                    *allow_print,
                    *allow_copy,
                    *allow_modify,
                ),
                _ => {
                    return Err(ToolError::Internal {
                        detail: "选项类型不匹配".to_string(),
                    })
                }
            };

        progress.report(0.2, "读取 PDF".to_string());
        check_cancel(cancel)?;

        let out = pdf_io::resolve_output(&req.output, req.conflict)?;

        if !encrypt {
            return decrypt(req, path, &out, progress, cancel);
        }

        // Leaving the owner password blank means "same as the user password".
        let owner_password = if owner_password.is_empty() {
            user_password.clone()
        } else {
            owner_password
        };

        let mut doc = pdf_io::open_for_edit(path, req.password.as_deref())?;
        let total = pdf_io::page_count(&doc);

        if doc.is_encrypted() {
            return Err(ToolError::UnsupportedFormat {
                path: Some(path.to_path_buf()),
                detail: "文件已加密，请先解密再重新加密".to_string(),
            });
        }

        progress.report(0.5, "应用 AES-256 加密".to_string());
        check_cancel(cancel)?;

        let mut permissions = Permissions::empty();
        if allow_print {
            permissions |= Permissions::PRINTABLE | Permissions::PRINTABLE_IN_HIGH_QUALITY;
        }
        if allow_copy {
            permissions |= Permissions::COPYABLE | Permissions::COPYABLE_FOR_ACCESSIBILITY;
        }
        if allow_modify {
            permissions |= Permissions::MODIFIABLE | Permissions::ANNOTABLE | Permissions::FILLABLE;
        }

        let params = EncryptionParams::new(&user_password, &owner_password)
            .algorithm(EncryptionAlgorithm::Aes256)
            .permissions(permissions);

        encrypt_document(&mut doc, &params).map_err(|e| ToolError::Internal {
            detail: format!("加密失败：{e}"),
        })?;

        progress.report(0.9, "写入 PDF".to_string());
        pdf_io::save_document(&mut doc, &out, req.conflict)?;

        // Invariants: artifact is encrypted, page count preserved, and it can be
        // reopened with the user password but not without one.
        validate::assert_encrypted(&out)?;
        validate::assert_page_count(&out, total, Some(&user_password))?;
        verify_password_paths(&out, &user_password)?;

        progress.report(1.0, "完成".to_string());
        Ok(ToolOutcome::success(
            format!("已使用 AES-256 加密 {total} 页"),
            vec![out],
        ))
    }
}

fn decrypt(
    req: &ToolRequest,
    path: &std::path::Path,
    out: &std::path::Path,
    progress: &Progress,
    cancel: &CancelFlag,
) -> Result<ToolOutcome, ToolError> {
    // Saying "not encrypted" is the honest answer for a plain file; the old
    // flow demanded a password it could not use.
    if !pdf_io::is_encrypted(path)? {
        return Err(ToolError::UnsupportedFormat {
            path: Some(path.to_path_buf()),
            detail: "文件未加密，无需解密".to_string(),
        });
    }
    let password = req
        .password
        .clone()
        .ok_or_else(|| ToolError::PasswordRequired {
            path: path.to_path_buf(),
        })?;

    progress.report(0.3, "读取加密 PDF".to_string());
    check_cancel(cancel)?;

    // `open_document` already authenticates and decrypts every object.
    let mut doc = pdf_io::open_document(path, Some(&password))?;
    let total = pdf_io::page_count(&doc);

    progress.report(0.8, "写入未加密 PDF".to_string());
    check_cancel(cancel)?;
    pdf_io::save_document(&mut doc, out, req.conflict)?;

    validate::assert_decrypted(out)?;
    validate::assert_page_count(out, total, None)?;

    progress.report(1.0, "完成".to_string());
    Ok(ToolOutcome::success(
        format!("已移除加密，共 {total} 页"),
        vec![out.to_path_buf()],
    ))
}

/// Prove the three acceptance paths: no password fails, wrong password fails,
/// correct password succeeds.
fn verify_password_paths(path: &std::path::Path, password: &str) -> Result<(), ToolError> {
    if lopdf::Document::load(path).is_ok() && !is_encrypted_on_disk(path) {
        return Err(ToolError::ValidationFailed {
            detail: "产物未加密，任何人可直接打开".to_string(),
        });
    }
    if lopdf::Document::load_with_password(path, "wrong-password-probe").is_ok() {
        return Err(ToolError::ValidationFailed {
            detail: "错误密码仍可打开产物".to_string(),
        });
    }
    if lopdf::Document::load_with_password(path, password).is_err() {
        return Err(ToolError::ValidationFailed {
            detail: "正确密码无法打开产物".to_string(),
        });
    }
    Ok(())
}

/// Cheap trailer check: does the file declare `/Encrypt`?
fn is_encrypted_on_disk(path: &std::path::Path) -> bool {
    match lopdf::Document::load(path) {
        Ok(doc) => doc.is_encrypted(),
        Err(_) => true,
    }
}
