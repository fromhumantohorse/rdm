# RDM privacy policy

RDM (the desktop application and its browser extension) collects no personal data.

- The extension sends the downloads you start (link, file name, page address, and the cookies and headers of that site needed to download the file) only to the RDM application on your own computer, through native messaging or a loopback connection (127.0.0.1). Nothing is sent to the developer or to any third party.
- No analytics, no telemetry, no advertising, no account.
- Cookies are passed to the local application in memory for the download only, and never written to disk.
- Settings (the list of captured file types) are stored locally in the browser.
- Optional VirusTotal scans, done by the desktop application only when you enable them with your own API key, send the file's SHA-256 (and the file only if you choose to upload it) to VirusTotal.

Contact: open an issue at https://github.com/vincentxjoubert-lang/rdm/issues
