const at = document.documentElement.dataset.at;
if (at) history.replaceState(null, "", at);
else if (location.search) history.replaceState(null, "", location.pathname);
