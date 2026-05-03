<!--
  HeadScripts Component
  Analytics, tracking pixels, and structured data for SEO
-->

<script lang="ts">
	import { getOrganizationSchema, getWebsiteSchema, getSoftwareSchema } from '$lib/utils/seo';

	// Generate structured data
	const organizationSchema = JSON.stringify(getOrganizationSchema());
	const websiteSchema = JSON.stringify(getWebsiteSchema());
	const softwareSchema = JSON.stringify(getSoftwareSchema());
</script>

<svelte:head>
	<!-- Primary Meta Tags -->
	<meta charset="utf-8" />
	<meta name="viewport" content="width=device-width, initial-scale=1" />
	<meta name="theme-color" content="#0a0f1a" />
	<meta name="color-scheme" content="dark" />

	<!-- Favicon -->
	<link rel="icon" type="image/svg+xml" href="/favicon.svg" />
	<link rel="icon" type="image/png" sizes="32x32" href="/favicon-32x32.png" />
	<link rel="icon" type="image/png" sizes="16x16" href="/favicon-16x16.png" />
	<link rel="apple-touch-icon" sizes="180x180" href="/apple-touch-icon.png" />
	<link rel="manifest" href="/site.webmanifest" />

	<!-- Preconnect to external domains -->
	<link rel="preconnect" href="https://fonts.googleapis.com" />
	<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin="anonymous" />
	<link rel="preconnect" href="https://api.fontshare.com" crossorigin="anonymous" />

	<!-- DNS Prefetch for analytics -->
	<link rel="dns-prefetch" href="https://www.googletagmanager.com" />
	<link rel="dns-prefetch" href="https://www.google-analytics.com" />

	<!-- Canonical URL fallback (pages should override) -->
	<link rel="canonical" href="https://nebo.dev" />

	<!-- Robots -->
	<meta
		name="robots"
		content="index, follow, max-image-preview:large, max-snippet:-1, max-video-preview:-1"
	/>
	<meta name="googlebot" content="index, follow" />

	<!-- Author and Copyright -->
	<meta name="author" content="Nebo" />
	<meta name="copyright" content="Nebo" />

	<!-- Language -->
	<meta property="og:locale" content="en_US" />
	<meta name="language" content="English" />

	<!-- Site Verification (add your IDs) -->
	<!-- <meta name="google-site-verification" content="YOUR_GOOGLE_VERIFICATION_ID" /> -->
	<!-- <meta name="msvalidate.01" content="YOUR_BING_VERIFICATION_ID" /> -->

	<!-- Structured Data: Organization -->
	{@html `<script type="application/ld+json">${organizationSchema}</script>`}

	<!-- Structured Data: Website -->
	{@html `<script type="application/ld+json">${websiteSchema}</script>`}

	<!-- Structured Data: Software Application -->
	{@html `<script type="application/ld+json">${softwareSchema}</script>`}

	<!-- Google Analytics (uncomment and add your ID when ready) -->
	<!--
	<script async src="https://www.googletagmanager.com/gtag/js?id=G-XXXXXXXXXX"></script>
	<script>
		window.dataLayer = window.dataLayer || [];
		function gtag() {
			dataLayer.push(arguments);
		}
		gtag('js', new Date());
		gtag('config', 'G-XXXXXXXXXX');
	</script>
	-->

	<!-- Meta Pixel (uncomment and add your ID when ready) -->
	<!--
	<script>
		!(function (f, b, e, v, n, t, s) {
			if (f.fbq) return;
			n = f.fbq = function () {
				n.callMethod ? n.callMethod.apply(n, arguments) : n.queue.push(arguments);
			};
			if (!f._fbq) f._fbq = n;
			n.push = n;
			n.loaded = !0;
			n.version = '2.0';
			n.queue = [];
			t = b.createElement(e);
			t.async = !0;
			t.src = v;
			s = b.getElementsByTagName(e)[0];
			s.parentNode.insertBefore(t, s);
		})(window, document, 'script', 'https://connect.facebook.net/en_US/fbevents.js');
		fbq('init', 'YOUR_PIXEL_ID');
		fbq('track', 'PageView');
	</script>
	-->
</svelte:head>

<!-- Meta Pixel noscript fallback (uncomment when ready) -->
<!--
<noscript>
	<img
		height="1"
		width="1"
		style="display:none"
		src="https://www.facebook.com/tr?id=YOUR_PIXEL_ID&ev=PageView&noscript=1"
		alt=""
	/>
</noscript>
-->
