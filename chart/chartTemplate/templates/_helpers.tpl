{{- define "igniteflux.name" -}}{{ .Release.Name }}{{- end -}}
{{- define "igniteflux.sa" -}}{{ if .Values.serviceAccount.create }}{{ include "igniteflux.name" . }}{{ else }}{{ .Values.serviceAccount.name }}{{ end }}{{- end -}}
