use std::{borrow::Cow, net::ToSocketAddrs};

use crate::pdu::{
    reader::read_pdu, writer::write_pdu, AssociationRJResult, AssociationRJSource, Pdu,
    PresentationContextProposed, PresentationContextResultReason,
};
use snafu::{ensure, OptionExt, ResultExt, Snafu};

use super::{Association, ServiceClassRole};

#[derive(Debug, Snafu)]
#[non_exhaustive]
pub enum Error {
    /// missing abstract syntax to begin negotiation
    MissingAbstractSyntax,

    /// could not connect to service class provider
    Connect { source: std::io::Error },

    /// failed to send association request
    SendRequest { source: crate::pdu::writer::Error },

    /// failed to receive association request
    ReceiveResponse { source: crate::pdu::reader::Error },

    #[snafu(display("unexpected response from SCP `{:?}`", pdu))]
    #[non_exhaustive]
    UnexpectedResponse {
        /// the PDU obtained from the server
        pdu: Pdu,
    },

    #[snafu(display("unknown response from SCP `{:?}`", pdu))]
    #[non_exhaustive]
    UnknownResponse {
        /// the PDU obtained from the server, of variant Unknown
        pdu: Pdu,
    },

    #[snafu(display("protocol version mismatch: expected {}, got {}", expected, got))]
    ProtocolVersionMismatch { expected: u16, got: u16 },

    /// the association was rejected by the service class provider
    Rejected {
        association_result: AssociationRJResult,
        association_source: AssociationRJSource,
    },

    /// no presentation contexts accepted by the service class provider
    NoAcceptedPresentationContexts,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A DICOM association builder for a service class user (SCU).
///
/// # Example
///
/// ```no_run
/// # use dicom_ul::association::scu::ScuAssociationOptions;
///
/// # fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let association = ScuAssociationOptions::new()
///    .with_abstract_syntax("1.2.840.10008.1.1")
///    .with_transfer_syntax("1.2.840.10008.1.2.1")
///    .establish("129.168.0.5:104")?;
/// # Ok(())
/// # }
/// ```
///
#[derive(Debug, Clone)]
pub struct ScuAssociationOptions {
    calling_aet: Cow<'static, str>,
    called_aet: Cow<'static, str>,
    application_context_name: Cow<'static, str>,
    abstract_syntax_uids: Vec<Cow<'static, str>>,
    transfer_syntax_uids: Vec<Cow<'static, str>>,
    protocol_version: u16,
    max_pdu_length: u32,
}

impl Default for ScuAssociationOptions {
    fn default() -> Self {
        ScuAssociationOptions {
            calling_aet: "CALLING-SCU".into(),
            called_aet: "ANY-SCP".into(),
            application_context_name: "1.2.840.10008.3.1.1.1".into(),
            abstract_syntax_uids: Vec::new(),
            transfer_syntax_uids: Vec::new(),
            protocol_version: 1,
            max_pdu_length: crate::pdu::reader::DEFAULT_MAX_PDU,
        }
    }
}

impl ScuAssociationOptions {
    /// Create a new set of options for establishing an association.
    pub fn new() -> Self {
        Self::default()
    }

    /// Include this abstract syntax
    /// in the list of proposed presentation contexts.
    pub fn with_abstract_syntax<T>(mut self, abstract_syntax_uid: T) -> Self
    where
        T: Into<Cow<'static, str>>,
    {
        self.abstract_syntax_uids.push(abstract_syntax_uid.into());
        self
    }

    /// Include this transfer syntax in each proposed presentation context.
    pub fn with_transfer_syntax<T>(mut self, transfer_syntax_uid: T) -> Self
    where
        T: Into<Cow<'static, str>>,
    {
        self.transfer_syntax_uids.push(transfer_syntax_uid.into());
        self
    }

    /// Override the maximum expected PDU length.
    pub fn max_pdu_length(mut self, value: u32) -> Self {
        self.max_pdu_length = value;
        self
    }

    /// Initiate the TCP connection and negotiate the
    pub fn establish<A: ToSocketAddrs>(self, address: A) -> Result<Association> {
        let ScuAssociationOptions {
            calling_aet,
            called_aet,
            application_context_name,
            abstract_syntax_uids,
            mut transfer_syntax_uids,
            protocol_version,
            max_pdu_length,
        } = self;

        // fail if no abstract syntaxes were provided: they represent intent,
        // should not be omitted by the user
        ensure!(!abstract_syntax_uids.is_empty(), MissingAbstractSyntax);

        // provide default transfer syntaxes
        if transfer_syntax_uids.is_empty() {
            // Explicit VR Little Endian
            transfer_syntax_uids.push("1.2.840.10008.1.2.1".into());
            // Implicit VR Little Endian
            transfer_syntax_uids.push("1.2.840.10008.1.2".into());
        }

        let presentation_contexts: Vec<_> = abstract_syntax_uids
            .into_iter()
            .enumerate()
            .map(|(i, abstract_syntax)| PresentationContextProposed {
                id: (i + 1) as u8,
                abstract_syntax: abstract_syntax.to_string(),
                transfer_syntaxes: transfer_syntax_uids
                    .iter()
                    .map(|uid| uid.to_string())
                    .collect(),
            })
            .collect();
        let msg = Pdu::AssociationRQ {
            protocol_version,
            calling_ae_title: calling_aet.to_string(),
            called_ae_title: called_aet.to_string(),
            application_context_name: application_context_name.to_string(),
            presentation_contexts: presentation_contexts.clone(),
            user_variables: vec![],
        };

        let mut socket = std::net::TcpStream::connect(address).context(Connect)?;

        // send request
        write_pdu(&mut socket, &msg).context(SendRequest)?;

        // receive response
        let msg = read_pdu(&mut socket, max_pdu_length).context(ReceiveResponse)?;

        match msg {
            Pdu::AssociationAC {
                protocol_version: protocol_version_scp,
                application_context_name: _,
                presentation_contexts: presentation_contexts_scp,
                user_variables: _,
            } => {
                ensure!(
                    protocol_version == protocol_version_scp,
                    ProtocolVersionMismatch {
                        expected: protocol_version,
                        got: protocol_version_scp,
                    }
                );

                let selected_context = presentation_contexts_scp
                    .into_iter()
                    .find(|c| c.reason == PresentationContextResultReason::Acceptance)
                    .context(NoAcceptedPresentationContexts)?;

                let presentation_context = presentation_contexts
                    .into_iter()
                    .find(|c| c.id == selected_context.id)
                    .context(NoAcceptedPresentationContexts)?;

                Ok(Association {
                    service_class_type: ServiceClassRole::Scu,
                    presentation_context_id: selected_context.id,
                    abstract_syntax_uid: presentation_context.abstract_syntax,
                    transfer_syntax_uid: selected_context.transfer_syntax,
                    max_pdu_length,
                    socket,
                })
            }
            Pdu::AssociationRJ { result, source } => Rejected {
                association_result: result,
                association_source: source,
            }
            .fail(),
            pdu @ Pdu::AbortRQ { .. }
            | pdu @ Pdu::ReleaseRQ { .. }
            | pdu @ Pdu::AssociationRQ { .. }
            | pdu @ Pdu::PData { .. }
            | pdu @ Pdu::ReleaseRP { .. } => UnexpectedResponse { pdu }.fail(),
            pdu @ Pdu::Unknown { .. } => UnknownResponse { pdu }.fail(),
        }
    }
}