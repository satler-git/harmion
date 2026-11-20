use crate::{
    Client, Error,
    protocol::resolution::{MergeInput, MergeInputRequest, MergePart, MergePlan},
};

impl Client {
    pub async fn merge_input(&self, request: MergeInputRequest) -> Result<MergeInput, Error> {
        let body = request.encode().map_err(|_| Error::InvalidRequest)?;
        let response = self.call("merge-input", &body).await?;
        serde_json::from_value(response).map_err(|_| Error::InvalidResponse)
    }

    /// Prepare an editable proposal. A subsequent explicit Resolve saves it.
    pub async fn merge_plan(&self, request: MergeInputRequest) -> Result<MergePlan, Error> {
        if !matches!(request.part, MergePart::Base(_)) {
            return Err(Error::InvalidRequest);
        }
        let (base_version, base) = match self.merge_input(request).await? {
            MergeInput::Body { version, text } => (version, text),
            MergeInput::ChooseBase { versions } => return Ok(MergePlan::ChooseBase { versions }),
            MergeInput::NoBase => return Ok(MergePlan::NoBase),
        };
        let (left, right) = tokio::try_join!(
            self.merge_input(MergeInputRequest {
                part: MergePart::Left,
                ..request
            }),
            self.merge_input(MergeInputRequest {
                part: MergePart::Right,
                ..request
            }),
        )?;
        let (MergeInput::Body { text: left, .. }, MergeInput::Body { text: right, .. }) =
            (left, right)
        else {
            return Err(Error::InvalidResponse);
        };
        tokio::task::spawn_blocking(move || {
            let (text, has_conflicts) = match diffy::merge(&base, &left, &right) {
                Ok(text) => (text, false),
                Err(text) => (text, true),
            };
            MergePlan::Proposed {
                base_version,
                text,
                has_conflicts,
            }
        })
        .await
        .map_err(|_| Error::InvalidResponse)
    }
}
