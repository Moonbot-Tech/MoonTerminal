(function () {
    "use strict";

    var labels = window.telegramLabels || {};
    function tr(key) { return labels[key] || ""; }
    function applyLang() {
        if (labels.locale) {
            document.documentElement.lang = labels.locale;
        }
    }
    applyLang();
    var webapp = window.Telegram && window.Telegram.WebApp;
    if (webapp) {
        webapp.ready();
        webapp.expand();
        if (webapp.setHeaderColor) {
            webapp.setHeaderColor("secondary_bg_color");
        }
    }

    function initData() {
        return (webapp && webapp.initData) || "";
    }

    var shellLine = document.getElementById("shell-line");
    if (shellLine) {
        shellLine.textContent = tr("mini_shell_checking");
    }
    fetch("/api/session", {
        method: "POST",
        headers: {
            "Content-Type": "application/json",
            "X-Telegram-Init-Data": initData()
        },
        body: "{}"
    }).then(function (response) {
        if (shellLine) {
            if (response.ok) {
                shellLine.textContent = tr("mini_shell_connected");
            } else if (response.status === 403) {
                shellLine.textContent = tr("mini_shell_denied");
            } else {
                shellLine.textContent = tr("mini_shell_unreachable");
            }
        }
    }).catch(function () {
        if (shellLine) {
            shellLine.textContent = tr("mini_shell_unreachable");
        }
    });
})();
