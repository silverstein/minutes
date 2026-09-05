"use client";

import { useEffect, useState } from "react";
import Script from "next/script";

// Preview deployments and local builds must not become production traffic.
export function WebsiteAnalytics() {
  const [enabled, setEnabled] = useState(false);
  useEffect(() => {
    setEnabled(["useminutes.app", "www.useminutes.app"].includes(window.location.hostname));
  }, []);
  if (!enabled) return null;

  return (
    <>
      <Script
        src="https://www.googletagmanager.com/gtag/js?id=G-998FBH4EMM"
        strategy="afterInteractive"
      />
      <Script id="google-analytics" strategy="afterInteractive">
        {`
          window.dataLayer = window.dataLayer || [];
          function gtag(){dataLayer.push(arguments);}
          // Consent Mode defaults, set before config so the first hit honors them.
          // No ads run on this site, so ad storage is denied everywhere. Analytics
          // cookies are denied by default in the EEA, UK, and Switzerland, where
          // consent is required first; GA still receives cookieless pings from
          // those visitors. Elsewhere analytics runs normally. No banner is shown.
          gtag('consent', 'default', {
            ad_storage: 'denied',
            ad_user_data: 'denied',
            ad_personalization: 'denied',
            analytics_storage: 'granted'
          });
          gtag('consent', 'default', {
            analytics_storage: 'denied',
            region: ['AT','BE','BG','HR','CY','CZ','DK','EE','FI','FR','DE','GR','HU',
                     'IE','IT','LV','LT','LU','MT','NL','PL','PT','RO','SK','SI','ES',
                     'SE','IS','LI','NO','GB','CH']
          });
          gtag('js', new Date());
          gtag('config', 'G-998FBH4EMM');
        `}
      </Script>
    </>
  );
}
