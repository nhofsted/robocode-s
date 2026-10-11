// CloudFront Function (viewer request, cloudfront-js-2.0) for the static site (web/README.md).
// - The data pages (/rankings, /bot, /pairing, /compare) render in the browser from 200.html.
//   Bot ids contain dots (/bot/sample.Walls), so they are matched by prefix, not by extension.
// - Other extensionless paths are prerendered pages: /docs/about → /docs/about.html.
// - A trailing slash redirects to the path without it (trailingSlash = 'never').
// Unknown paths get 200.html with status 404 from the distribution's error responses.
function handler(event) {
	var request = event.request;
	var uri = request.uri;
	if (uri === '/') {
		request.uri = '/index.html';
		return request;
	}
	if (uri.endsWith('/')) {
		var query = Object.keys(request.querystring)
			.map(function (k) {
				var v = request.querystring[k];
				return v.value === '' ? k : k + '=' + v.value;
			})
			.join('&');
		var location = uri.replace(/\/+$/, '') || '/';
		return {
			statusCode: 301,
			statusDescription: 'Moved Permanently',
			headers: { location: { value: query ? location + '?' + query : location } }
		};
	}
	if (/^\/(rankings|bot|pairing|compare)(\/|$)/.test(uri)) {
		request.uri = '/200.html';
		return request;
	}
	if (uri.slice(uri.lastIndexOf('/') + 1).indexOf('.') === -1) request.uri = uri + '.html';
	return request;
}
