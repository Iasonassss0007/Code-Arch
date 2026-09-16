# Route callers

Backend views and the frontend files whose request strings name every segment of the view's route (mount prefix such as `api/` dropped). Static evidence: string-built URLs can be missed or over-matched.

## `BulkDownloadView` — `src/documents/views.py`

Routes: `documents/bulk_download`

- `src-ui/src/app/services/rest/document.service.ts`

## `BulkEditObjectsView` — `src/documents/views.py`

Routes: `bulk_edit_objects`

- `src-ui/src/app/services/rest/abstract-name-filter-service.ts`

## `BulkEditView` — `src/documents/views.py`

Routes: `documents/bulk_edit`

- `src-ui/src/app/services/rest/document.service.ts`

## `ChatStreamingView` — `src/documents/views.py`

Routes: `documents/chat`

- `src-ui/src/app/services/chat.service.ts`

## `CorrespondentViewSet` — `src/documents/views.py`

Routes: `correspondents`

- `src-ui/src/app/services/rest/correspondent.service.ts`

## `CustomFieldViewSet` — `src/documents/views.py`

Routes: `custom_fields`

- `src-ui/src/app/components/document-detail/document-detail.component.ts`
- `src-ui/src/app/services/rest/custom-fields.service.ts`

## `DeleteDocumentsView` — `src/documents/views.py`

Routes: `documents/delete`

- `src-ui/src/app/services/rest/document.service.ts`

## `DocumentTypeViewSet` — `src/documents/views.py`

Routes: `document_types`

- `src-ui/src/app/services/rest/document-type.service.ts`

## `EditPdfDocumentsView` — `src/documents/views.py`

Routes: `documents/edit_pdf`

- `src-ui/src/app/services/rest/document.service.ts`

## `GlobalSearchView` — `src/documents/views.py`

Routes: `search`

- `src-ui/src/app/services/rest/document.service.ts`
- `src-ui/src/app/services/rest/search.service.ts`

## `LogViewSet` — `src/documents/views.py`

Routes: `logs`

- `src-ui/src/app/services/rest/log.service.ts`

## `MergeDocumentsAsVersionsView` — `src/documents/views.py`

Routes: `documents/merge_as_versions`

- `src-ui/src/app/services/rest/document.service.ts`

## `MergeDocumentsView` — `src/documents/views.py`

Routes: `documents/merge`

- `src-ui/src/app/services/rest/document.service.ts`

## `PostDocumentView` — `src/documents/views.py`

Routes: `documents/post_document`

- `src-ui/src/app/services/rest/document.service.ts`

## `RemoteVersionView` — `src/documents/views.py`

Routes: `remote_version`

- `src-ui/src/app/services/rest/remote-version.service.ts`

## `RemovePasswordDocumentsView` — `src/documents/views.py`

Routes: `documents/remove_password`

- `src-ui/src/app/services/rest/document.service.ts`

## `ReprocessDocumentsView` — `src/documents/views.py`

Routes: `documents/reprocess`

- `src-ui/src/app/services/rest/document.service.ts`

## `RotateDocumentsView` — `src/documents/views.py`

Routes: `documents/rotate`

- `src-ui/src/app/services/rest/document.service.ts`

## `SavedViewViewSet` — `src/documents/views.py`

Routes: `saved_views`

- `src-ui/src/app/services/rest/saved-view.service.ts`

## `SearchAutoCompleteView` — `src/documents/views.py`

Routes: `search/autocomplete`

- `src-ui/src/app/services/rest/search.service.ts`

## `SelectionDataView` — `src/documents/views.py`

Routes: `documents/selection_data`

- `src-ui/src/app/services/rest/document.service.ts`

## `ShareLinkBundleViewSet` — `src/documents/views.py`

Routes: `share_link_bundles`

- `src-ui/src/app/services/rest/share-link-bundle.service.ts`

## `ShareLinkViewSet` — `src/documents/views.py`

Routes: `share_links`

- `src-ui/src/app/services/rest/share-link.service.ts`

## `SharedLinkView` — `src/documents/views.py`

Routes: `share`

- `src-ui/src/app/components/common/share-link-bundle-dialog/share-link-bundle-dialog.component.ts`
- `src-ui/src/app/components/common/share-link-bundle-manage-dialog/share-link-bundle-manage-dialog.component.ts`
- `src-ui/src/app/components/common/share-links-dialog/share-links-dialog.component.ts`

## `StatisticsView` — `src/documents/views.py`

Routes: `statistics`

- `src-ui/src/app/components/dashboard/widgets/statistics-widget/statistics-widget.component.ts`

## `StoragePathViewSet` — `src/documents/views.py`

Routes: `storage_paths`

- `src-ui/src/app/services/rest/storage-path.service.ts`

## `SystemStatusView` — `src/documents/views.py`

Routes: `status`

- `src-ui/src/app/components/common/share-link-bundle-dialog/share-link-bundle-dialog.component.ts`
- `src-ui/src/app/services/system-status.service.ts`

## `TagViewSet` — `src/documents/views.py`

Routes: `tags`

- `src-ui/src/app/components/document-detail/document-detail.component.ts`
- `src-ui/src/app/services/rest/tag.service.ts`

## `TasksViewSet` — `src/documents/views.py`

Routes: `tasks`

- `src-ui/src/app/components/app-frame/app-frame.component.ts`
- `src-ui/src/app/services/tasks.service.ts`

## `TrashView` — `src/documents/views.py`

Routes: `trash`

- `src-ui/src/app/services/trash.service.ts`

## `UiSettingsView` — `src/documents/views.py`

Routes: `ui_settings`

- `src-ui/src/app/services/settings.service.ts`

## `UnifiedSearchViewSet` — `src/documents/views.py`

Routes: `documents`

- `src-ui/src/app/components/document-detail/document-detail.component.ts`
- `src-ui/src/app/services/chat.service.ts`
- `src-ui/src/app/services/rest/document-notes.service.ts`
- `src-ui/src/app/services/rest/document.service.ts`
- `src-ui/src/app/services/rest/share-link.service.ts`
- `src-ui/src/app/services/trash.service.ts`

## `WorkflowViewSet` — `src/documents/views.py`

Routes: `workflows`

- `src-ui/src/app/services/rest/workflow.service.ts`

## `serve_logo` — `src/documents/views.py`

Routes: `logo`

- `src-ui/src/app/components/app-frame/app-frame.component.ts`
- `src-ui/src/app/components/common/logo/logo.component.ts`

## `ApplicationConfigurationViewSet` — `src/paperless/views.py`

Routes: `config`

- `src-ui/src/app/services/config.service.ts`

## `DisconnectSocialAccountView` — `src/paperless/views.py`

Routes: `profile/disconnect_social_account`

- `src-ui/src/app/services/profile.service.ts`

## `GenerateAuthTokenView` — `src/paperless/views.py`

Routes: `profile/generate_auth_token`

- `src-ui/src/app/services/profile.service.ts`

## `GroupViewSet` — `src/paperless/views.py`

Routes: `groups`

- `src-ui/src/app/services/rest/group.service.ts`

## `ProfileView` — `src/paperless/views.py`

Routes: `profile`

- `src-ui/src/app/services/profile.service.ts`

## `SocialAccountProvidersView` — `src/paperless/views.py`

Routes: `profile/social_account_providers`

- `src-ui/src/app/services/profile.service.ts`

## `TOTPView` — `src/paperless/views.py`

Routes: `profile/totp`

- `src-ui/src/app/services/profile.service.ts`

## `UserViewSet` — `src/paperless/views.py`

Routes: `users`

- `src-ui/src/app/services/rest/user.service.ts`

## `MailAccountViewSet` — `src/paperless_mail/views.py`

Routes: `mail_accounts`

- `src-ui/src/app/services/rest/mail-account.service.ts`

## `MailRuleViewSet` — `src/paperless_mail/views.py`

Routes: `mail_rules`

- `src-ui/src/app/services/rest/mail-rule.service.ts`

## `ProcessedMailViewSet` — `src/paperless_mail/views.py`

Routes: `processed_mail`

- `src-ui/src/app/services/rest/processed-mail.service.ts`
