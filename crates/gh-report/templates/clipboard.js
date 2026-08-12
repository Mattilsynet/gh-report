function tidyGovernanceShowStatus(statusEl, message) {
    statusEl.textContent = message;
}

function tidyGovernanceCopy(button) {
    const targetId = button.getAttribute("data-copy-target");
    const target = document.getElementById(targetId);
    const statusEl = document.getElementById("tidy-governance-copy-status");
    if (!target || !statusEl) {
        return;
    }
    if (!window.isSecureContext || !navigator.clipboard) {
        tidyGovernanceShowStatus(
            statusEl,
            "Clipboard copy requires HTTPS or localhost — select the text above and copy it by hand."
        );
        return;
    }
    navigator.clipboard.writeText(target.value).then(
        () => tidyGovernanceShowStatus(statusEl, "Copied to clipboard."),
        () => tidyGovernanceShowStatus(statusEl, "Copy failed — select the text above and copy it by hand.")
    );
}

document.querySelectorAll("[data-copy-target]").forEach((button) => {
    button.addEventListener("click", () => tidyGovernanceCopy(button));
});
